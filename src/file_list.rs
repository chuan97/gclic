//! The compare view's selectable list of changed files, plus the file row
//! drawing shared with the other panels.

use crate::app::Action;
use crate::git::{DiffSource, FileEntry};
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub struct FileList {
    a: String,
    b: String,
    files: Vec<FileEntry>,
    selected: usize,
    scroll: usize,
}

impl FileList {
    pub fn new(a: String, b: String, files: Vec<FileEntry>) -> FileList {
        FileList { a, b, files, selected: 0, scroll: 0 }
    }

    pub fn set_files(&mut self, files: Vec<FileEntry>) {
        self.files = files;
        self.selected = self.selected.min(self.files.len().saturating_sub(1));
    }

    /// Going the other way, an added file is a deleted one and the reverse.
    pub fn swap_refs(&mut self) {
        std::mem::swap(&mut self.a, &mut self.b);
        for file in &mut self.files {
            file.status = match file.status {
                'A' => 'D',
                'D' => 'A',
                other => other,
            };
        }
    }

    #[cfg(test)]
    pub fn statuses(&self) -> Vec<char> {
        self.files.iter().map(|file| file.status).collect()
    }

    pub fn selected_source(&self) -> Option<DiffSource> {
        self.source_at(self.selected)
    }

    /// The diffs one key away, most likely first, for prefetching.
    pub fn sources_around(&self) -> Vec<DiffSource> {
        let positions = ui::one_key_away(self.selected, self.files.len());
        positions.into_iter().filter_map(|index| self.source_at(index)).collect()
    }

    fn source_at(&self, index: usize) -> Option<DiffSource> {
        let path = self.files.get(index)?.path.clone();
        Some(DiffSource::Refs { a: self.a.clone(), b: self.b.clone(), path })
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        let last = self.files.len().saturating_sub(1);
        let before = self.selected;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.selected = ui::next_index(self.selected, self.files.len()),
            KeyCode::Char('k') | KeyCode::Up => self.selected = ui::previous_index(self.selected, self.files.len()),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = last,
            KeyCode::Enter => {
                return match self.selected_source() {
                    Some(source) => Action::OpenDiff(source),
                    None => Action::None,
                };
            }
            _ => {}
        }
        if self.selected == before {
            return Action::None;
        }
        match self.selected_source() {
            Some(source) => Action::PreviewDiff(source),
            None => Action::None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        if self.files.is_empty() {
            let note = Line::styled("No files changed", Style::default().fg(ui::DIM));
            frame.render_widget(note, area);
            return;
        }
        self.scroll = ui::keep_visible(self.scroll, self.selected, area.height as usize);
        let visible = self.files.iter().enumerate().skip(self.scroll);
        let lines = visible
            .map(|(i, file)| file_line(file, i == self.selected, focused))
            .collect();
        ui::render_lines(frame, area, lines);
    }
}

/// One `M path/to/file` row, shared with the staging panel.
pub fn file_line(file: &FileEntry, selected: bool, focused: bool) -> Line<'_> {
    let letter = Span::styled(
        format!(" {} ", file.status),
        Style::default().fg(ui::status_color(file.status)),
    );
    let line = Line::from(vec![letter, Span::raw(file.path.as_str())]);
    if selected && focused {
        line.style(Style::default().bg(ui::SELECTED_BG).add_modifier(Modifier::BOLD))
    } else if selected {
        line.style(Style::default().bg(ui::SELECTED_BG))
    } else {
        line
    }
}
