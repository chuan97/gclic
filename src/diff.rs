//! The side-by-side diff pane, used by every feature.

use crate::git::{self, DiffSource};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

const REMOVED_BG: Color = Color::Indexed(52);
const ADDED_BG: Color = Color::Indexed(22);
const FILLER_BG: Color = Color::Indexed(235);

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub number: usize,
    pub text: String,
    pub changed: bool,
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
            removed.push(Cell { number: old_number, text: clean(content), changed: true });
            old_number += 1;
        } else if let Some(content) = line.strip_prefix('+') {
            added.push(Cell { number: new_number, text: clean(content), changed: true });
            new_number += 1;
        } else if !line.starts_with('\\') {
            // Context line. `\ No newline at end of file` is ignored.
            flush_change(&mut rows, &mut removed, &mut added);
            let content = clean(line.strip_prefix(' ').unwrap_or(line));
            rows.push(Row::Lines {
                left: Some(Cell { number: old_number, text: content.clone(), changed: false }),
                right: Some(Cell { number: new_number, text: content, changed: false }),
            });
            old_number += 1;
            new_number += 1;
        }
    }
    flush_change(&mut rows, &mut removed, &mut added);
    rows
}

/// Pairs held-back lines row by row. The shorter side gets blank fillers.
fn flush_change(rows: &mut Vec<Row>, removed: &mut Vec<Cell>, added: &mut Vec<Cell>) {
    let mut left = removed.drain(..);
    let mut right = added.drain(..);
    loop {
        let (l, r) = (left.next(), right.next());
        if l.is_none() && r.is_none() {
            break;
        }
        rows.push(Row::Lines { left: l, right: r });
    }
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
    scroll: usize,
    height: usize,
}

impl DiffPane {
    pub fn load(source: DiffSource) -> Result<DiffPane> {
        let rows = parse_diff(&git::diff(&source)?);
        Ok(DiffPane { source, rows, scroll: 0, height: 0 })
    }

    /// Fetches the diff again, for files that changed while it was on screen.
    pub fn reload(&mut self) -> Result<()> {
        self.rows = parse_diff(&git::diff(&self.source)?);
        self.scroll = self.scroll.min(self.rows.len().saturating_sub(1));
        Ok(())
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

    pub fn handle_key(&mut self, key: KeyEvent) {
        let last = self.rows.len().saturating_sub(1);
        let half_page = (self.height / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('d') if ctrl => self.scroll = (self.scroll + half_page).min(last),
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(half_page),
            KeyCode::Char('j') | KeyCode::Down => self.scroll = (self.scroll + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('G') => self.scroll = last,
            KeyCode::Char('n') => {
                let next = (self.scroll + 1..self.rows.len()).find(|&i| self.is_heading(i));
                self.scroll = next.unwrap_or(self.scroll);
            }
            KeyCode::Char('p') => {
                let previous = (0..self.scroll).rev().find(|&i| self.is_heading(i));
                self.scroll = previous.unwrap_or(self.scroll);
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
        let visible = self.rows.iter().skip(self.scroll).take(self.height);
        for (i, row) in visible.enumerate() {
            let y = inner.y + i as u16;
            let full = Rect { y, height: 1, ..inner };
            match row {
                Row::Heading(text) => {
                    let line = Line::styled(text.as_str(), Style::default().fg(Color::Cyan));
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
                    frame.render_widget(side_line(left.as_ref(), REMOVED_BG), left_area);
                    frame.render_widget(Line::styled("│", Style::default().fg(ui::DIM)), separator);
                    frame.render_widget(side_line(right.as_ref(), ADDED_BG), right_area);
                }
            }
        }
    }
}

fn side_line(cell: Option<&Cell>, changed_bg: Color) -> Line<'_> {
    let Some(cell) = cell else {
        return Line::default().style(Style::default().bg(FILLER_BG));
    };
    let gutter = Span::styled(format!("{:>4} ", cell.number), Style::default().fg(ui::DIM));
    let line = Line::from(vec![gutter, Span::raw(cell.text.as_str())]);
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
        Some(Cell { number, text: text.to_string(), changed })
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
