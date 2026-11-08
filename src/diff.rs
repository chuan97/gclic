//! The side-by-side diff pane, used by every feature.

use crate::git::{self, DiffSource};
use crate::highlight::{self, ColoredLine};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::ops::Range;

// Muted tints rather than pure red and green, so the syntax colours on top
// stay readable. Changed words get a stronger tint of the same colour.
const REMOVED_BG: Color = Color::Rgb(58, 30, 34);
const ADDED_BG: Color = Color::Rgb(28, 52, 38);
const REMOVED_WORD_BG: Color = Color::Rgb(112, 48, 54);
const ADDED_WORD_BG: Color = Color::Rgb(46, 96, 62);
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

fn changed_cell(number: usize, content: &str) -> Cell {
    Cell { number, text: clean(content), changed: true, marks: Vec::new(), colors: Vec::new() }
}

/// Gives each line its syntax colours. They come from the whole old and new
/// files, as a hunk on its own can start inside a comment or a string. Slow
/// for big files, so the worker thread does it.
pub fn highlight_rows(rows: &mut [Row], source: &DiffSource) {
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
        return; // Nothing to colour, as in a binary file's diff.
    }
    let (old, new) = git::file_versions(source);
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

pub struct DiffPane {
    pub source: DiffSource,
    /// The diff as git printed it, to tell whether a reload changed anything.
    text: String,
    rows: Vec<Row>,
    /// The row under the cursor. The view keeps it in the middle, except near
    /// the ends, where it moves on without scrolling further.
    active: usize,
    height: usize,
    /// How many characters each side's text is scrolled to the left.
    shift: usize,
    /// Room for text on each side, past the line numbers.
    text_width: usize,
    /// Whether the rows have their syntax colours yet. The worker thread
    /// makes them, and `apply_colors` takes them.
    colored: bool,
}

/// The most rows the colour cache keeps, over all its diffs. Enough for the
/// diffs around the cursor and a good few recent ones, in a few MB.
const CACHE_ROWS: usize = 20_000;

/// Diffs already coloured, so that going back to one, or to one prefetched,
/// shows its colours straight away. The most recently used come last.
#[derive(Default)]
pub struct ColorCache {
    entries: Vec<(DiffSource, String, Vec<Row>)>,
}

impl ColorCache {
    /// The coloured rows of this diff, if they were made from this same text.
    pub fn get(&mut self, source: &DiffSource, text: &str) -> Option<Vec<Row>> {
        let position = self.entries.iter().position(|(s, t, _)| s == source && t == text)?;
        let entry = self.entries.remove(position);
        let rows = entry.2.clone();
        self.entries.push(entry);
        Some(rows)
    }

    /// Whether some version of this diff is cached. A working-tree file may
    /// have changed since, which showing it will find out.
    pub fn contains(&self, source: &DiffSource) -> bool {
        self.entries.iter().any(|(s, ..)| s == source)
    }

    /// Adds a diff, replacing an older version of it and dropping the least
    /// recently used ones past the limit.
    pub fn insert(&mut self, source: DiffSource, text: String, rows: Vec<Row>) {
        self.entries.retain(|(s, ..)| *s != source);
        self.entries.push((source, text, rows));
        let total = |entries: &[(DiffSource, String, Vec<Row>)]| -> usize {
            entries.iter().map(|(.., rows)| rows.len()).sum()
        };
        while self.entries.len() > 1 && total(&self.entries) > CACHE_ROWS {
            self.entries.remove(0);
        }
    }
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
    /// Loads without syntax colours, which can take a moment for a big file.
    /// They come from the cache or the worker thread, via `apply_colors`.
    pub fn load(source: DiffSource) -> Result<DiffPane> {
        let text = git::diff(&source)?;
        let rows = parse_diff(&text);
        Ok(DiffPane {
            source,
            text,
            rows,
            active: 0,
            height: 0,
            shift: 0,
            text_width: 0,
            colored: false,
        })
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn needs_colors(&self) -> bool {
        !self.colored
    }

    /// Takes coloured rows, if they were made from the diff on screen. Ones
    /// for an older version of it are dropped.
    pub fn apply_colors(&mut self, text: &str, rows: Vec<Row>) {
        if self.text == text {
            self.rows = rows;
            self.colored = true;
        }
    }

    /// Fetches the diff again, for files that changed while it was on screen.
    pub fn reload(&mut self) -> Result<()> {
        self.set_text(git::diff(&self.source)?);
        Ok(())
    }

    /// Takes a newer version of the diff, which needs colouring again. Nothing
    /// happens if it hasn't changed, as is usual on a refresh.
    pub fn set_text(&mut self, text: String) {
        if text == self.text {
            return;
        }
        self.rows = parse_diff(&text);
        self.colored = false;
        self.text = text;
        self.active = self.active.min(self.rows.len().saturating_sub(1));
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
            KeyCode::Char('N') => {
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

    // The visible text, cut wherever a syntax colour or a changed word starts
    // or ends, so each piece has one text colour and one background.
    let text = &cell.text;
    let start = text.char_indices().nth(shift).map_or(text.len(), |(i, _)| i);
    let mut cuts = vec![start, text.len()];
    let edges = cell.marks.iter().chain(cell.colors.iter().map(|(range, _)| range));
    cuts.extend(edges.flat_map(|range| [range.start, range.end]).filter(|&cut| cut > start));
    cuts.sort_unstable();
    cuts.dedup();
    for piece in cuts.windows(2) {
        let (from, to) = (piece[0], piece[1].min(text.len()));
        if from >= to {
            continue;
        }
        let mut style = Style::default();
        if let Some((_, color)) = cell.colors.iter().find(|(range, _)| range.contains(&from)) {
            style = style.fg(*color);
        }
        if cell.marks.iter().any(|mark| mark.contains(&from)) {
            style = style.bg(word_bg);
        }
        spans.push(Span::styled(&text[from..to], style));
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
        Some(Cell { number, text: text.to_string(), changed, marks: Vec::new(), colors: Vec::new() })
    }

    fn marked<'a>(text: &'a str, marks: &[Range<usize>]) -> Vec<&'a str> {
        marks.iter().map(|mark| &text[mark.clone()]).collect()
    }

    fn pane_with_rows(count: usize, height: usize) -> DiffPane {
        DiffPane {
            source: DiffSource::Unstaged("f".to_string()),
            text: String::new(),
            rows: vec![Row::Heading("x".to_string()); count],
            active: 0,
            height,
            shift: 0,
            text_width: 0,
            colored: true,
        }
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn scrolling_sideways_keeps_the_line_number_and_the_marks() {
        let cell = Cell {
            number: 7,
            text: "héllo wörld".to_string(),
            changed: true,
            marks: vec![7..13],
            colors: Vec::new(),
        };
        let line = side_line(Some(&cell), 3, false, ADDED_BG, ADDED_WORD_BG);
        assert_eq!(text_of(&line), "   7 lo wörld");
        let marked = line.spans.iter().find(|span| span.style.bg == Some(ADDED_WORD_BG));
        assert_eq!(marked.unwrap().content, "wörld");
    }

    #[test]
    fn syntax_colours_and_changed_words_combine() {
        let (red, blue) = (Color::Rgb(255, 0, 0), Color::Rgb(0, 0, 255));
        let cell = Cell {
            number: 1,
            text: "let x = 1;".to_string(),
            changed: true,
            marks: vec![8..9],
            colors: vec![(0..3, red), (3..10, blue)],
        };
        let line = side_line(Some(&cell), 0, false, ADDED_BG, ADDED_WORD_BG);
        let pieces: Vec<_> = line.spans[1..]
            .iter()
            .map(|span| (span.content.as_ref(), span.style.fg, span.style.bg))
            .collect();
        assert_eq!(
            pieces,
            [
                ("let", Some(red), None),
                (" x = ", Some(blue), None),
                ("1", Some(blue), Some(ADDED_WORD_BG)),
                (";", Some(blue), None),
            ]
        );
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
    fn scrolling_past_a_short_line_leaves_only_its_number() {
        let cell = Cell { number: 1, text: "ab".to_string(), changed: false, marks: Vec::new(), colors: Vec::new() };
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
    fn the_cache_only_matches_the_same_text() {
        let mut cache = ColorCache::default();
        let source = DiffSource::Unstaged("f".to_string());
        cache.insert(source.clone(), "v1".to_string(), parse_diff("@@ -1 +1 @@\n-a\n+b"));
        assert!(cache.get(&source, "v1").is_some());
        assert!(cache.get(&source, "v2").is_none());
        assert!(cache.contains(&source));
    }

    #[test]
    fn the_cache_drops_the_least_recently_used_past_its_limit() {
        let mut cache = ColorCache::default();
        let file = |name: &str| DiffSource::Unstaged(name.to_string());
        let rows = vec![Row::Heading("x".to_string()); CACHE_ROWS / 2];
        cache.insert(file("a"), String::new(), rows.clone());
        cache.insert(file("b"), String::new(), rows.clone());
        cache.get(&file("a"), ""); // a is now the most recently used
        cache.insert(file("c"), String::new(), rows);
        assert!(cache.contains(&file("a")) && cache.contains(&file("c")));
        assert!(!cache.contains(&file("b")));
    }

    #[test]
    fn colours_for_another_version_of_the_diff_are_dropped() {
        let mut pane = pane_with_rows(1, 10);
        pane.text = "new".to_string();
        pane.colored = false;
        pane.apply_colors("old", Vec::new());
        assert!(pane.needs_colors());
        pane.apply_colors("new", Vec::new());
        assert!(!pane.needs_colors());
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
