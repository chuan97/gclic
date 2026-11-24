//! Diffs and conflicts parsed into rows, a removed line beside the added one
//! it became, with the words that changed marked, and syntax colours on them.

use crate::git::{self, DiffSource};
use crate::highlight::{self, ColoredLine};
use ratatui::style::Color;
use std::ops::Range;

/// A changed line sharing less than this with its partner is a rewrite, not an
/// edit, and marking most of it would only add noise.
const MIN_SIMILARITY: f32 = 0.3;

/// Comparing two lines costs tokens x tokens, so very long ones are skipped.
const MAX_TOKEN_PAIRS: usize = 250_000;

/// Pairing lines compares every removed line with every added one, so bigger
/// changes than this many comparisons are paired by position instead.
const MAX_LINE_PAIRS: usize = 400;

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub number: usize,
    pub text: String,
    pub changed: bool,
    /// Byte ranges of `text` that differ from the line opposite.
    pub marks: Vec<Range<usize>>,
    /// The syntax colour of each byte range of `text`. Empty if not known.
    pub colors: Vec<(Range<usize>, Color)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    /// A hunk header, or a note such as "binary file".
    Heading(String),
    Lines { left: Option<Cell>, right: Option<Cell> },
}

/// Turns a unified diff of one file into side-by-side rows.
pub fn parse_diff(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut in_hunk = false;
    let (mut old_number, mut new_number) = (0, 0);
    // A change is a run of removed lines followed by a run of added lines.
    // They are held back here so they can be lined up with each other.
    let mut removed: Vec<Cell> = Vec::new();
    let mut added: Vec<Cell> = Vec::new();

    for line in text.lines() {
        if line.starts_with("@@ ") {
            flush_change(&mut rows, &mut removed, &mut added);
            (old_number, new_number) = parse_hunk_header(line);
            rows.push(Row::Heading(line.to_string()));
            in_hunk = true;
        } else if !in_hunk {
            if line.starts_with("Binary files ") {
                rows.push(Row::Heading("Binary file".to_string()));
            }
        } else if let Some(content) = line.strip_prefix('-') {
            // A removal after additions starts a new change.
            if !added.is_empty() {
                flush_change(&mut rows, &mut removed, &mut added);
            }
            removed.push(changed_cell(old_number, content));
            old_number += 1;
        } else if let Some(content) = line.strip_prefix('+') {
            added.push(changed_cell(new_number, content));
            new_number += 1;
        } else if !line.starts_with('\\') {
            // Context line. `\ No newline at end of file` is ignored.
            flush_change(&mut rows, &mut removed, &mut added);
            let content = clean(line.strip_prefix(' ').unwrap_or(line));
            let context = |number, text| Cell {
                number,
                text,
                changed: false,
                marks: Vec::new(),
                colors: Vec::new(),
            };
            rows.push(Row::Lines {
                left: Some(context(old_number, content.clone())),
                right: Some(context(new_number, content)),
            });
            old_number += 1;
            new_number += 1;
        }
    }
    flush_change(&mut rows, &mut removed, &mut added);
    rows
}

/// The rows for a diff's text: a unified diff, or for a conflict the file
/// itself, with its markers.
pub fn rows_for(source: &DiffSource, text: &str) -> Vec<Row> {
    match source {
        DiffSource::Conflict(_) => parse_conflict(text),
        _ => parse_diff(text),
    }
}

/// Shows a file with merge conflicts as its two sides: lines outside the
/// conflicts as context, and each conflict as yours on the left and theirs on
/// the right, under a heading, paired as a diff's lines are. Line numbers are
/// the file's own, markers included, so opening it in an editor at the active
/// line lands on the conflict. A diff3 base section, if any, is left out.
pub fn parse_conflict(text: &str) -> Vec<Row> {
    if text.is_empty() {
        return vec![Row::Heading("Deleted on one side: stage or remove it to resolve".to_string())];
    }
    enum Part {
        Outside,
        Ours,
        Base,
        Theirs,
    }
    let mut rows = Vec::new();
    let mut part = Part::Outside;
    let (mut ours, mut theirs) = (Vec::new(), Vec::new());
    let mut ours_label = String::new();
    let mut conflicts = 0;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        match part {
            Part::Outside if line.starts_with("<<<<<<<") => {
                ours_label = line.trim_start_matches('<').trim().to_string();
                part = Part::Ours;
            }
            Part::Outside => {
                let text = clean(line);
                let context = |text: String| Cell { number, text, changed: false, marks: Vec::new(), colors: Vec::new() };
                rows.push(Row::Lines { left: Some(context(text.clone())), right: Some(context(text)) });
            }
            Part::Ours | Part::Base if line.starts_with("=======") => part = Part::Theirs,
            Part::Ours if line.starts_with("|||||||") => part = Part::Base,
            Part::Ours => ours.push(changed_cell(number, line)),
            Part::Base => {}
            Part::Theirs if line.starts_with(">>>>>>>") => {
                conflicts += 1;
                let theirs_label = line.trim_start_matches('>').trim();
                rows.push(Row::Heading(format!("Conflict {conflicts}: {ours_label} ↔ {theirs_label}")));
                flush_change(&mut rows, &mut ours, &mut theirs);
                part = Part::Outside;
            }
            Part::Theirs => theirs.push(changed_cell(number, line)),
        }
    }
    // A conflict cut off by the end of the file still shows what it has.
    flush_change(&mut rows, &mut ours, &mut theirs);
    if conflicts == 0 {
        rows.insert(0, Row::Heading("No conflicts left: stage the file to mark it resolved".to_string()));
    } else {
        // Now that the total is known, every heading can say "n of total".
        let mut seen = 0;
        for row in &mut rows {
            if let Row::Heading(heading) = row
                && heading.starts_with("Conflict ")
            {
                seen += 1;
                *heading = heading.replacen(&format!("Conflict {seen}:"), &format!("Conflict {seen} of {conflicts}:"), 1);
            }
        }
    }
    rows
}

fn changed_cell(number: usize, content: &str) -> Cell {
    Cell { number, text: clean(content), changed: true, marks: Vec::new(), colors: Vec::new() }
}

/// Gives each line its syntax colours. They come from the whole old and new
/// files, as a hunk on its own can start inside a comment or a string. Slow
/// for big files, so the worker thread does it. Returns the length of the file
/// the minimap measures, which reading them finds out on the way.
pub fn highlight_rows(rows: &mut [Row], source: &DiffSource) -> usize {
    let last_line = |side: fn(&Row) -> Option<&Cell>| {
        rows.iter().filter_map(side).map(|cell| cell.number).max().unwrap_or(0)
    };
    let last_old = last_line(|row| match row {
        Row::Lines { left, .. } => left.as_ref(),
        Row::Heading(_) => None,
    });
    let last_new = last_line(|row| match row {
        Row::Lines { right, .. } => right.as_ref(),
        Row::Heading(_) => None,
    });
    if last_old == 0 && last_new == 0 {
        return 0; // Nothing to colour, as in a binary file's diff.
    }
    let (old, new) = git::file_versions(source);
    let measured = if measures_new_side(rows) { &new } else { &old };
    let file_lines = measured.as_ref().map_or(0, |text| text.lines().count());
    let colored = |text: Option<String>, last: usize| {
        text.map(|text| highlight::highlight(source.path(), &text, last)).unwrap_or_default()
    };
    let (old, new) = (colored(old, last_old), colored(new, last_new));
    for row in rows {
        if let Row::Lines { left, right } = row {
            color_cell(left.as_mut(), &old);
            color_cell(right.as_mut(), &new);
        }
    }
    file_lines
}

/// Only takes the colours when the file's line is the one in the diff, in case
/// the two don't match, say because the file changed between reading them.
fn color_cell(cell: Option<&mut Cell>, lines: &[ColoredLine]) {
    let Some(cell) = cell else {
        return;
    };
    let line = cell.number.checked_sub(1).and_then(|index| lines.get(index));
    if let Some(line) = line
        && line.text == cell.text
    {
        cell.colors = line.colors.clone();
    }
}

/// Lines up held-back lines, pairing each removed line with the added line it
/// most resembles. Lines left between two pairs share rows as far as they go,
/// and the shorter side gets blank fillers.
fn flush_change(rows: &mut Vec<Row>, removed: &mut Vec<Cell>, added: &mut Vec<Cell>) {
    let (total_left, total_right) = (removed.len(), added.len());
    let pairs = pair_lines(removed, added);
    let mut left = removed.drain(..);
    let mut right = added.drain(..);
    let (mut next_left, mut next_right) = (0, 0);
    // The last, made-up pair at the ends flushes the lines after the real ones.
    for (i, j) in pairs.into_iter().chain([(total_left, total_right)]) {
        for k in 0..(i - next_left).max(j - next_right) {
            let l = if next_left + k < i { left.next() } else { None };
            let r = if next_right + k < j { right.next() } else { None };
            push_row(rows, l, r);
        }
        if i < total_left {
            push_row(rows, left.next(), right.next());
        }
        (next_left, next_right) = (i + 1, j + 1);
    }
}

fn push_row(rows: &mut Vec<Row>, mut left: Option<Cell>, mut right: Option<Cell>) {
    if let (Some(l), Some(r)) = (&mut left, &mut right) {
        (l.marks, r.marks) = word_marks(&l.text, &r.text);
    }
    rows.push(Row::Lines { left, right });
}

/// Which removed line goes with which added line, as index pairs. Each pair
/// is similar enough to mark its changed words, both sides stay in order, and
/// the pairs are the ones with the highest total similarity. So a line added
/// above an edited one doesn't pair with it and shift every pair below.
fn pair_lines(removed: &[Cell], added: &[Cell]) -> Vec<(usize, usize)> {
    let (m, n) = (removed.len(), added.len());
    if m * n > MAX_LINE_PAIRS {
        return (0..m.min(n)).map(|i| (i, i)).collect();
    }
    let similar: Vec<Vec<Option<f32>>> = removed
        .iter()
        .map(|old| {
            added
                .iter()
                .map(|new| Some(line_similarity(&old.text, &new.text)).filter(|s| *s >= MIN_SIMILARITY))
                .collect()
        })
        .collect();

    // best[i][j] is the highest total similarity pairing removed[i..] with added[j..].
    let mut best = vec![vec![0.0f32; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            let skip = best[i + 1][j].max(best[i][j + 1]);
            best[i][j] = match similar[i][j] {
                Some(score) => skip.max(score + best[i + 1][j + 1]),
                None => skip,
            };
        }
    }

    // Walks the choices that gave each best total, from the top.
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < m && j < n {
        if similar[i][j].is_some_and(|score| best[i][j] == score + best[i + 1][j + 1]) {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if best[i][j] == best[i + 1][j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// Splits a line into words and single other characters, as byte ranges.
fn tokenize(text: &str) -> Vec<Range<usize>> {
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut in_word = false;
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        match tokens.last_mut() {
            Some(last) if in_word && is_word(ch) => last.end = end,
            _ => tokens.push(start..end),
        }
        in_word = is_word(ch);
    }
    tokens
}

/// Two lines split into tokens, with which of them belong to the lines'
/// longest common subsequence.
struct TokenMatch {
    old: Vec<Range<usize>>,
    new: Vec<Range<usize>>,
    old_in_common: Vec<bool>,
    new_in_common: Vec<bool>,
}

/// None when the lines are too long to compare.
fn match_tokens(old: &str, new: &str) -> Option<TokenMatch> {
    let (a, b) = (tokenize(old), tokenize(new));
    if a.len() * b.len() > MAX_TOKEN_PAIRS {
        return None;
    }
    let same = |i: usize, j: usize| old[a[i].clone()] == new[b[j].clone()];

    // lengths[i][j] is the length of the longest match between a[i..] and b[j..].
    let mut lengths = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lengths[i][j] = if same(i, j) {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }

    let (mut in_common_a, mut in_common_b) = (vec![false; a.len()], vec![false; b.len()]);
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if same(i, j) {
            in_common_a[i] = true;
            in_common_b[j] = true;
            i += 1;
            j += 1;
        } else if lengths[i + 1][j] >= lengths[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some(TokenMatch { old: a, new: b, old_in_common: in_common_a, new_in_common: in_common_b })
}

/// How much of two lines they share, from 0 to 1. Whitespace doesn't count,
/// or any two lines of code would look related.
fn similarity(old: &str, new: &str, matched: &TokenMatch) -> f32 {
    let visible = |text: &str| text.chars().filter(|ch| !ch.is_whitespace()).count();
    let common: usize = matched
        .old
        .iter()
        .zip(&matched.old_in_common)
        .filter(|(_, in_common)| **in_common)
        .map(|(token, _)| visible(&old[token.clone()]))
        .sum();
    let total = visible(old) + visible(new);
    if total == 0 { 1.0 } else { (2 * common) as f32 / total as f32 }
}

fn line_similarity(old: &str, new: &str) -> f32 {
    match_tokens(old, new).map_or(0.0, |matched| similarity(old, new, &matched))
}

/// Finds which words differ between two versions of a line. Returns the marks
/// for each side, or none if the lines have too little in common.
fn word_marks(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let Some(matched) = match_tokens(old, new) else {
        return (Vec::new(), Vec::new());
    };
    if similarity(old, new, &matched) < MIN_SIMILARITY {
        return (Vec::new(), Vec::new());
    }
    (
        merged_marks(&matched.old, &matched.old_in_common),
        merged_marks(&matched.new, &matched.new_in_common),
    )
}

/// The tokens not in common, with neighbours joined into one range.
fn merged_marks(tokens: &[Range<usize>], in_common: &[bool]) -> Vec<Range<usize>> {
    let mut marks: Vec<Range<usize>> = Vec::new();
    for (token, common) in tokens.iter().zip(in_common) {
        if *common {
            continue;
        }
        match marks.last_mut() {
            Some(last) if last.end == token.start => last.end = token.end,
            _ => marks.push(token.clone()),
        }
    }
    marks
}

/// Reads the start line numbers from `@@ -12,5 +12,6 @@ context`.
fn parse_hunk_header(line: &str) -> (usize, usize) {
    let mut parts = line.split_whitespace().skip(1);
    let mut start = |prefix: char| -> usize {
        parts
            .next()
            .and_then(|range| range.strip_prefix(prefix))
            .and_then(|range| range.split(',').next())
            .and_then(|number| number.parse().ok())
            .unwrap_or(0)
    };
    let old = start('-');
    let new = start('+');
    (old, new)
}

/// Tabs would break column alignment, as cells are drawn one character each.
fn clean(text: &str) -> String {
    text.replace('\t', "    ")
}

/// Whether the minimap measures the new version of the file: the one the
/// diff goes to, unless it has no lines, as when the file was deleted.
pub(super) fn measures_new_side(rows: &[Row]) -> bool {
    rows.iter().any(|row| matches!(row, Row::Lines { right: Some(_), .. }))
}

#[cfg(test)]
// Marks are lists of ranges, so a list holding one range is meant.
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;
    use crate::diff::testing::cell;

    fn marked<'a>(text: &'a str, marks: &[Range<usize>]) -> Vec<&'a str> {
        marks.iter().map(|mark| &text[mark.clone()]).collect()
    }

    #[test]
    fn a_line_that_differs_from_the_file_gets_no_colours() {
        let lines = [ColoredLine { text: "other".to_string(), colors: vec![(0..5, Color::Red)] }];
        let mut cell = cell(1, "mine", false).unwrap();
        color_cell(Some(&mut cell), &lines);
        assert!(cell.colors.is_empty());
        let mut cell = Cell { text: "other".to_string(), ..cell };
        color_cell(Some(&mut cell), &lines);
        assert_eq!(cell.colors.len(), 1);
    }

    #[test]
    fn a_conflict_shows_yours_against_theirs_with_the_files_line_numbers() {
        let file = "a\n<<<<<<< HEAD\nreturn 3\n=======\nreturn 2\n>>>>>>> other\nz\n";
        let rows = parse_conflict(file);
        assert_eq!(rows[1], Row::Heading("Conflict 1 of 1: HEAD ↔ other".to_string()));
        let Row::Lines { left: Some(ours), right: Some(theirs) } = &rows[2] else { panic!() };
        assert_eq!((ours.text.as_str(), ours.number), ("return 3", 3));
        assert_eq!((theirs.text.as_str(), theirs.number), ("return 2", 5));
        assert_eq!(marked(&ours.text, &ours.marks), ["3"]);
        assert_eq!(texts(&rows[3..]), [(Some("z"), Some("z"))]);
    }

    #[test]
    fn a_diff3_base_is_left_out_and_conflicts_are_counted() {
        let file = "<<<<<<< ours\nx\n||||||| base\nold\n=======\ny\n>>>>>>> theirs\n\
                    <<<<<<< ours\np\n=======\nq\n>>>>>>> theirs\n";
        let rows = parse_conflict(file);
        let headings: Vec<_> = rows.iter().filter(|row| matches!(row, Row::Heading(_))).collect();
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[1], &Row::Heading("Conflict 2 of 2: ours ↔ theirs".to_string()));
        assert!(!texts(&rows).iter().any(|(left, right)| *left == Some("old") || *right == Some("old")));
    }

    #[test]
    fn a_resolved_or_deleted_file_says_so() {
        let resolved = parse_conflict("all fine\n");
        assert!(matches!(&resolved[0], Row::Heading(h) if h.starts_with("No conflicts left")));
        assert!(matches!(&parse_conflict("")[0], Row::Heading(h) if h.starts_with("Deleted on one side")));
    }

    fn texts(rows: &[Row]) -> Vec<(Option<&str>, Option<&str>)> {
        rows.iter()
            .filter_map(|row| match row {
                Row::Lines { left, right } => Some((
                    left.as_ref().map(|c| c.text.as_str()),
                    right.as_ref().map(|c| c.text.as_str()),
                )),
                Row::Heading(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_line_added_above_an_edit_does_not_shift_the_pairing() {
        let diff = "@@ -1,2 +1,3 @@\n-let total = count + 1;\n+log(\"start\");\n+let total = count + 2;\n print(total);";
        let rows = parse_diff(diff);
        assert_eq!(
            texts(&rows),
            [
                (None, Some("log(\"start\");")),
                (Some("let total = count + 1;"), Some("let total = count + 2;")),
                (Some("print(total);"), Some("print(total);")),
            ]
        );
        let Row::Lines { left: Some(left), right: Some(right) } = &rows[2] else { panic!() };
        assert_eq!(marked(&left.text, &left.marks), ["1"]);
        assert_eq!(marked(&right.text, &right.marks), ["2"]);
    }

    #[test]
    fn unrelated_lines_still_share_a_row() {
        let rows = parse_diff("@@ -1 +1 @@\n-completely different\n+nothing shared here");
        assert_eq!(texts(&rows), [(Some("completely different"), Some("nothing shared here"))]);
    }

    #[test]
    fn a_removed_line_between_edits_stands_alone() {
        let diff = "@@ -1,3 +1,2 @@\n-let a = 1;\n-obsolete();\n-let b = 2;\n+let a = 10;\n+let b = 20;";
        assert_eq!(
            texts(&parse_diff(diff)),
            [
                (Some("let a = 1;"), Some("let a = 10;")),
                (Some("obsolete();"), None),
                (Some("let b = 2;"), Some("let b = 20;")),
            ]
        );
    }

    #[test]
    fn big_changes_are_paired_by_position() {
        let cells = |text: &str| -> Vec<Cell> {
            (0..21).map(|i| changed_cell(i, &format!("{text} {i}"))).collect()
        };
        let pairs = pair_lines(&cells("old"), &cells("new"));
        assert_eq!(pairs, (0..21).map(|i| (i, i)).collect::<Vec<_>>());
    }

    #[test]
    fn marks_only_the_words_that_changed() {
        let (old, new) = ("let total = count + 1;", "let total = amount + 2;");
        let (left, right) = word_marks(old, new);
        assert_eq!(marked(old, &left), ["count", "1"]);
        assert_eq!(marked(new, &right), ["amount", "2"]);
    }

    #[test]
    fn marks_an_insertion_on_one_side_only() {
        let (old, new) = ("line 2", "line 2 changed");
        let (left, right) = word_marks(old, new);
        assert!(left.is_empty());
        assert_eq!(marked(new, &right), [" changed"]);
    }

    #[test]
    fn neighbouring_changed_tokens_become_one_mark() {
        let (old, new) = ("value(b)", "value[c]");
        let (_, right) = word_marks(old, new);
        assert_eq!(marked(new, &right), ["[c]"]);
    }

    #[test]
    fn rewritten_lines_get_no_marks() {
        let (left, right) = word_marks("completely different", "nothing shared here");
        assert!(left.is_empty() && right.is_empty());
    }

    #[test]
    fn marks_respect_multibyte_characters() {
        let (old, new) = ("héllo wörld ok", "héllo wörld fine");
        let (left, right) = word_marks(old, new);
        assert_eq!(marked(old, &left), ["ok"]);
        assert_eq!(marked(new, &right), ["fine"]);
    }

    #[test]
    fn paired_lines_get_marks_when_parsed() {
        let rows = parse_diff("@@ -1 +1 @@\n-let b = 2;\n+let b = 3;\n");
        let Row::Lines { left: Some(left), right: Some(right) } = &rows[1] else {
            panic!("expected a paired row")
        };
        assert_eq!(marked(&left.text, &left.marks), ["2"]);
        assert_eq!(marked(&right.text, &right.marks), ["3"]);
    }

    #[test]
    fn pairs_removals_with_additions_and_pads_the_shorter_side() {
        let rows = parse_diff(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -10,3 +10,4 @@ fn x\n ctx\n-old\n+new1\n+new2\n end\n",
        );
        assert_eq!(
            rows,
            vec![
                Row::Heading("@@ -10,3 +10,4 @@ fn x".to_string()),
                Row::Lines { left: cell(10, "ctx", false), right: cell(10, "ctx", false) },
                Row::Lines { left: cell(11, "old", true), right: cell(11, "new1", true) },
                Row::Lines { left: None, right: cell(12, "new2", true) },
                Row::Lines { left: cell(12, "end", false), right: cell(13, "end", false) },
            ]
        );
    }

    #[test]
    fn content_that_looks_like_a_file_header_stays_content() {
        let rows = parse_diff("@@ -1 +1 @@\n--- removed\n+++ added\n");
        assert_eq!(
            rows[1],
            Row::Lines { left: cell(1, "-- removed", true), right: cell(1, "++ added", true) }
        );
    }

    #[test]
    fn ignores_the_no_newline_marker() {
        let rows = parse_diff("@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n");
        assert_eq!(rows[1], Row::Lines { left: cell(1, "a", true), right: cell(1, "b", true) });
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn reports_binary_files() {
        let rows = parse_diff("diff --git a/x b/x\nBinary files a/x and b/x differ\n");
        assert_eq!(rows, vec![Row::Heading("Binary file".to_string())]);
    }

    #[test]
    fn new_file_hunk_starts_at_line_one() {
        let rows = parse_diff("@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(rows[1], Row::Lines { left: None, right: cell(1, "a", true) });
        assert_eq!(rows[2], Row::Lines { left: None, right: cell(2, "b", true) });
    }
}
