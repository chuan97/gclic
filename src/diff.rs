//! The side-by-side diff pane, used by every feature.

use crate::git::{self, DiffSource};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::ops::Range;

const REMOVED_BG: Color = Color::Indexed(52);
const ADDED_BG: Color = Color::Indexed(22);
const REMOVED_WORD_BG: Color = Color::Indexed(88);
const ADDED_WORD_BG: Color = Color::Indexed(28);
const FILLER_BG: Color = Color::Indexed(235);
/// A changed line sharing less than this with its partner is a rewrite, not an
/// edit, and marking most of it would only add noise.
const MIN_SIMILARITY: f32 = 0.3;
/// Comparing two lines costs tokens x tokens, so very long ones are skipped.
const MAX_TOKEN_PAIRS: usize = 250_000;
/// Pairing lines compares every removed line with every added one, so bigger
/// changes than this many comparisons are paired by position instead.
const MAX_LINE_PAIRS: usize = 400;
/// Columns moved by one sideways scroll.
const SIDE_STEP: usize = 4;
/// The line number in front of each side's text, as drawn by `side_line`.
const GUTTER_WIDTH: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub number: usize,
    pub text: String,
    pub changed: bool,
    /// Byte ranges of `text` that differ from the line opposite.
    pub marks: Vec<Range<usize>>,
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
            let context = |number, text| Cell { number, text, changed: false, marks: Vec::new() };
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

fn changed_cell(number: usize, content: &str) -> Cell {
    Cell { number, text: clean(content), changed: true, marks: Vec::new() }
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

pub struct DiffPane {
    pub source: DiffSource,
    rows: Vec<Row>,
    /// The row under the cursor. The view keeps it in the middle, except near
    /// the ends, where it moves on without scrolling further.
    active: usize,
    height: usize,
    /// How many characters each side's text is scrolled to the left.
    shift: usize,
    /// Room for text on each side, past the line numbers.
    text_width: usize,
}

/// The length, in characters, of the longest line on either side.
fn longest_line(rows: &[Row]) -> usize {
    let cells = rows.iter().flat_map(|row| match row {
        Row::Lines { left, right } => [left.as_ref(), right.as_ref()],
        Row::Heading(_) => [None, None],
    });
    cells.flatten().map(|cell| cell.text.chars().count()).max().unwrap_or(0)
}

impl DiffPane {
    pub fn load(source: DiffSource) -> Result<DiffPane> {
        let rows = parse_diff(&git::diff(&source)?);
        Ok(DiffPane { source, rows, active: 0, height: 0, shift: 0, text_width: 0 })
    }

    /// Fetches the diff again, for files that changed while it was on screen.
    pub fn reload(&mut self) -> Result<()> {
        self.rows = parse_diff(&git::diff(&self.source)?);
        self.active = self.active.min(self.rows.len().saturating_sub(1));
        Ok(())
    }

    /// The first row on screen: the one that centres the active row, without
    /// going past either end.
    fn scroll(&self) -> usize {
        let furthest = self.rows.len().saturating_sub(self.height);
        self.active.saturating_sub(self.height / 2).min(furthest)
    }

    /// The line number in the current file at the active row. Rows without
    /// one, such as hunk headings and removed lines, take the next line's, or
    /// the previous one's at the very end.
    pub fn active_line(&self) -> Option<usize> {
        let new_line = |row: &Row| match row {
            Row::Lines { right: Some(cell), .. } => Some(cell.number),
            _ => None,
        };
        let below = self.rows.iter().skip(self.active).find_map(new_line);
        below.or_else(|| self.rows.iter().take(self.active).rev().find_map(new_line))
    }

    /// Back to the start of the lines, for when the diff loses focus, as h and l
    /// stop scrolling it then.
    pub fn reset_shift(&mut self) {
        self.shift = 0;
    }

    /// The furthest sideways scroll that still shows the end of the longest line.
    fn max_shift(&self) -> usize {
        longest_line(&self.rows).saturating_sub(self.text_width)
    }

    fn title(&self) -> String {
        match &self.source {
            DiffSource::Unstaged(path) => format!(" {path} "),
            DiffSource::Untracked(path) => format!(" {path} (untracked) "),
            DiffSource::Staged(path) => format!(" {path} (staged) "),
            DiffSource::Commit { sha, path } => format!(" {path} @ {} ", &sha[..sha.len().min(7)]),
            DiffSource::Refs { a, b, path } => format!(" {path}: {a} → {b} "),
        }
    }

    /// Moves the active row, stopping at the ends.
    pub fn handle_key(&mut self, key: KeyEvent) {
        let last = self.rows.len().saturating_sub(1);
        let half_page = (self.height / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('d') if ctrl => self.active = (self.active + half_page).min(last),
            KeyCode::Char('u') if ctrl => self.active = self.active.saturating_sub(half_page),
            KeyCode::Char('j') | KeyCode::Down => self.active = (self.active + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.active = self.active.saturating_sub(1),
            KeyCode::Char('g') => self.active = 0,
            KeyCode::Char('G') => self.active = last,
            KeyCode::Char('h') | KeyCode::Left => self.shift = self.shift.saturating_sub(SIDE_STEP),
            KeyCode::Char('l') | KeyCode::Right => {
                self.shift = (self.shift + SIDE_STEP).min(self.max_shift());
            }
            KeyCode::Char('n') => {
                let next = (self.active + 1..self.rows.len()).find(|&i| self.is_heading(i));
                self.active = next.unwrap_or(self.active);
            }
            KeyCode::Char('p') => {
                let previous = (0..self.active).rev().find(|&i| self.is_heading(i));
                self.active = previous.unwrap_or(self.active);
            }
            _ => {}
        }
    }

    fn is_heading(&self, index: usize) -> bool {
        matches!(self.rows[index], Row::Heading(_))
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = ui::panel(self.title(), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        self.height = inner.height as usize;

        if self.rows.is_empty() {
            let note = Line::styled("No changes", Style::default().fg(ui::DIM));
            frame.render_widget(note, inner);
            return;
        }

        let column_width = inner.width.saturating_sub(1) / 2;
        // Both sides get the same room, as the right one only gains a spare column.
        self.text_width = (column_width as usize).saturating_sub(GUTTER_WIDTH);
        // The pane may have widened since the last scroll.
        self.shift = self.shift.min(self.max_shift());
        let scroll = self.scroll();
        let visible = self.rows.iter().enumerate().skip(scroll).take(self.height);
        for (i, (index, row)) in visible.enumerate() {
            let y = inner.y + i as u16;
            let full = Rect { y, height: 1, ..inner };
            // Like a list's selection, the active row only shows while focused.
            let active = focused && index == self.active;
            match row {
                Row::Heading(text) => {
                    let line = Line::styled(text.as_str(), Style::default().fg(Color::Cyan));
                    let line = if active { line.style(Style::default().bg(ui::SELECTED_BG)) } else { line };
                    frame.render_widget(line, full);
                }
                Row::Lines { left, right } => {
                    let left_area = Rect { width: column_width, ..full };
                    let separator = Rect { x: full.x + column_width, width: 1, ..full };
                    let right_area = Rect {
                        x: full.x + column_width + 1,
                        width: inner.width.saturating_sub(column_width + 1),
                        ..full
                    };
                    let left = side_line(left.as_ref(), self.shift, active, REMOVED_BG, REMOVED_WORD_BG);
                    let right = side_line(right.as_ref(), self.shift, active, ADDED_BG, ADDED_WORD_BG);
                    frame.render_widget(left, left_area);
                    frame.render_widget(Line::styled("│", Style::default().fg(ui::DIM)), separator);
                    frame.render_widget(right, right_area);
                }
            }
        }
    }
}

/// One side of a row, with its text scrolled `shift` characters to the left.
/// The line number stays put, and stands out on the active row. Only the number
/// does, so that the colours of a change stay readable.
fn side_line(
    cell: Option<&Cell>,
    shift: usize,
    active: bool,
    changed_bg: Color,
    word_bg: Color,
) -> Line<'_> {
    let Some(cell) = cell else {
        return Line::default().style(Style::default().bg(FILLER_BG));
    };
    let gutter_style = if active {
        Style::default().fg(Color::White).bg(ui::SELECTED_BG).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ui::DIM)
    };
    let gutter = Span::styled(format!("{:>4} ", cell.number), gutter_style);
    let mut spans = vec![gutter];

    // The text as plain and marked pieces, cut where the visible part starts.
    let text = &cell.text;
    let start = text.char_indices().nth(shift).map_or(text.len(), |(i, _)| i);
    let mut pieces = Vec::new();
    let mut position = 0;
    for mark in &cell.marks {
        pieces.push((position..mark.start, Style::default()));
        pieces.push((mark.clone(), Style::default().bg(word_bg)));
        position = mark.end;
    }
    pieces.push((position..text.len(), Style::default()));
    for (range, style) in pieces {
        let visible = range.start.max(start)..range.end;
        if !visible.is_empty() {
            spans.push(Span::styled(&text[visible], style));
        }
    }
    let line = Line::from(spans);
    if cell.changed {
        line.style(Style::default().bg(changed_bg))
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(number: usize, text: &str, changed: bool) -> Option<Cell> {
        Some(Cell { number, text: text.to_string(), changed, marks: Vec::new() })
    }

    fn marked<'a>(text: &'a str, marks: &[Range<usize>]) -> Vec<&'a str> {
        marks.iter().map(|mark| &text[mark.clone()]).collect()
    }

    fn pane_with_rows(count: usize, height: usize) -> DiffPane {
        DiffPane {
            source: DiffSource::Unstaged("f".to_string()),
            rows: vec![Row::Heading("x".to_string()); count],
            active: 0,
            height,
            shift: 0,
            text_width: 0,
        }
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn scrolling_sideways_keeps_the_line_number_and_the_marks() {
        let cell = Cell { number: 7, text: "héllo wörld".to_string(), changed: true, marks: vec![7..13] };
        let line = side_line(Some(&cell), 3, false, ADDED_BG, ADDED_WORD_BG);
        assert_eq!(text_of(&line), "   7 lo wörld");
        let marked = line.spans.iter().find(|span| span.style.bg == Some(ADDED_WORD_BG));
        assert_eq!(marked.unwrap().content, "wörld");
    }

    #[test]
    fn scrolling_past_a_short_line_leaves_only_its_number() {
        let cell = Cell { number: 1, text: "ab".to_string(), changed: false, marks: Vec::new() };
        assert_eq!(text_of(&side_line(Some(&cell), 10, false, ADDED_BG, ADDED_WORD_BG)), "   1 ");
    }

    #[test]
    fn scrolling_sideways_stops_once_the_longest_line_ends() {
        let mut pane = pane_with_rows(1, 10);
        pane.rows = vec![Row::Lines { left: cell(1, &"x".repeat(10), false), right: None }];
        pane.text_width = 4;
        for _ in 0..5 {
            pane.handle_key(KeyEvent::from(KeyCode::Char('l')));
        }
        assert_eq!(pane.shift, 6);
        pane.handle_key(KeyEvent::from(KeyCode::Char('h')));
        assert_eq!(pane.shift, 2);
    }

    #[test]
    fn the_active_row_stops_at_both_ends() {
        let mut pane = pane_with_rows(5, 2);
        pane.handle_key(KeyEvent::from(KeyCode::Char('k')));
        assert_eq!(pane.active, 0);
        pane.handle_key(KeyEvent::from(KeyCode::Char('G')));
        pane.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(pane.active, 4);
    }

    #[test]
    fn the_view_centres_the_active_row_except_near_the_ends() {
        let mut pane = pane_with_rows(20, 6);
        pane.active = 10;
        assert_eq!(pane.scroll(), 7);
        pane.active = 1;
        assert_eq!(pane.scroll(), 0);
        pane.active = 19;
        assert_eq!(pane.scroll(), 14);
    }

    #[test]
    fn a_diff_that_fits_on_screen_doesnt_scroll() {
        let mut pane = pane_with_rows(2, 10);
        pane.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(pane.scroll(), 0);
    }

    #[test]
    fn the_active_line_is_the_next_one_in_the_current_file() {
        let mut pane = pane_with_rows(0, 10);
        pane.rows = parse_diff("@@ -4,2 +4,1 @@\n-gone\n same\n-also gone");
        // The heading and a removed line both lead to `same`, line 4 now.
        assert_eq!(pane.active_line(), Some(4));
        pane.active = 1;
        assert_eq!(pane.active_line(), Some(4));
        // At the end, with nothing below, it takes the line above.
        pane.active = 3;
        assert_eq!(pane.active_line(), Some(4));
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
