//! Feature 1: the staging area, modelled on VS Code's git panel.

use crate::app::{Action, Confirm};
use crate::file_list::file_line;
use crate::git::{self, DiffSource, FileEntry, Status};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

/// Also read by the app, which writes key marks right after it.
pub const TITLE: &str = " Changes ";

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Staged,
    Changes,
}

pub struct StagingPanel {
    status: Status,
    /// Index into the staged files followed by the unstaged files.
    selected: usize,
    scroll: usize,
}

impl StagingPanel {
    pub fn new() -> StagingPanel {
        StagingPanel { status: Status::default(), selected: 0, scroll: 0 }
    }

    pub fn has_staged(&self) -> bool {
        !self.status.staged.is_empty()
    }

    fn total(&self) -> usize {
        self.status.staged.len() + self.status.unstaged.len()
    }

    fn selection(&self) -> Option<(Section, &FileEntry)> {
        let staged = self.status.staged.len();
        if self.selected < staged {
            Some((Section::Staged, &self.status.staged[self.selected]))
        } else {
            let entry = self.status.unstaged.get(self.selected - staged)?;
            Some((Section::Changes, entry))
        }
    }

    pub fn selected_source(&self) -> Option<DiffSource> {
        let (section, entry) = self.selection()?;
        let path = entry.path.clone();
        Some(match (section, entry.status) {
            (Section::Staged, _) => DiffSource::Staged(path),
            (Section::Changes, '?') => DiffSource::Untracked(path),
            (Section::Changes, _) => DiffSource::Unstaged(path),
        })
    }

    /// Replaces the file lists, keeping the selection on the same file if it
    /// is still there. If not (it was just staged, say) the selection stays at
    /// the same position in its section, so pressing the stage key repeatedly
    /// walks down the list.
    pub fn set_status(&mut self, status: Status) {
        let previous = self
            .selection()
            .map(|(section, entry)| (section, entry.path.clone()));
        let old_staged = self.status.staged.len();
        self.status = status;
        let new_staged = self.status.staged.len();

        if let Some((section, path)) = previous {
            let (list, old_offset, new_offset) = match section {
                Section::Staged => (&self.status.staged, 0, 0),
                Section::Changes => (&self.status.unstaged, old_staged, new_staged),
            };
            if let Some(position) = list.iter().position(|e| e.path == path) {
                self.selected = new_offset + position;
            } else if !list.is_empty() {
                self.selected = new_offset + (self.selected - old_offset).min(list.len() - 1);
            }
        }
        self.selected = self.selected.min(self.total().saturating_sub(1));
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.set_status(git::status()?);
        Ok(())
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        let last = self.total().saturating_sub(1);
        let before = self.selected;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.selected = ui::next_index(self.selected, self.total()),
            KeyCode::Char('k') | KeyCode::Up => self.selected = ui::previous_index(self.selected, self.total()),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = last,
            KeyCode::Enter => {
                return match self.selected_source() {
                    Some(source) => Action::OpenDiff(source),
                    None => Action::None,
                };
            }
            KeyCode::Char(' ') | KeyCode::Char('s') => return self.toggle_stage(),
            KeyCode::Char('a') => return Action::after_change(git::stage_all()),
            KeyCode::Char('A') => return Action::after_change(git::unstage_all()),
            KeyCode::Char('d') => return self.ask_to_restore(),
            KeyCode::Char('c') => {
                return if self.status.staged.is_empty() {
                    Action::Message("Nothing staged to commit".to_string())
                } else {
                    Action::OpenCommitBox
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

    fn toggle_stage(&self) -> Action {
        match self.selection() {
            Some((Section::Staged, entry)) => Action::after_change(git::unstage(&entry.path)),
            Some((Section::Changes, entry)) => Action::after_change(git::stage(&entry.path)),
            None => Action::None,
        }
    }

    fn ask_to_restore(&self) -> Action {
        match self.selection() {
            Some((Section::Staged, _)) => {
                Action::Message("Unstage the file before restoring it".to_string())
            }
            Some((Section::Changes, entry)) if entry.status == '?' => {
                Action::Confirm(Confirm::DeleteUntracked(entry.path.clone()))
            }
            Some((Section::Changes, entry)) => {
                Action::Confirm(Confirm::Restore(entry.path.clone()))
            }
            None => Action::None,
        }
    }

    /// The rows it asks for: both lists with their headings, or one line if
    /// there's nothing to show.
    pub fn wanted_height(&self) -> u16 {
        match self.total() {
            0 => 1,
            files => (files + 2) as u16,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let staged = self.status.staged.len();
        let block = ui::panel(TITLE.to_string(), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.total() == 0 {
            let note = Line::styled("No changes", Style::default().fg(ui::DIM));
            frame.render_widget(note, inner);
            return;
        }

        let heading = Style::default().fg(ui::DIM).add_modifier(Modifier::BOLD);
        let mut lines = vec![Line::styled(format!("Staged ({staged})"), heading)];
        for (i, entry) in self.status.staged.iter().enumerate() {
            lines.push(file_line(entry, i == self.selected, focused));
        }
        let unstaged = self.status.unstaged.len();
        lines.push(Line::styled(format!("Changes ({unstaged})"), heading));
        for (i, entry) in self.status.unstaged.iter().enumerate() {
            lines.push(file_line(entry, staged + i == self.selected, focused));
        }

        // Selected row on screen: one heading above, two once past the staged files.
        let headings = if self.selected < staged { 1 } else { 2 };
        let selected_row = self.selected + headings;
        self.scroll = ui::keep_visible(self.scroll, selected_row, inner.height as usize);
        let lines = lines.into_iter().skip(self.scroll).collect();
        ui::render_lines(frame, inner, lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(paths: &[&str]) -> Vec<FileEntry> {
        paths
            .iter()
            .map(|path| FileEntry { status: 'M', path: path.to_string() })
            .collect()
    }

    fn panel(staged: &[&str], unstaged: &[&str], selected: usize) -> StagingPanel {
        StagingPanel {
            status: Status { staged: entries(staged), unstaged: entries(unstaged) },
            selected,
            scroll: 0,
        }
    }

    fn selected_path(panel: &StagingPanel) -> &str {
        &panel.selection().unwrap().1.path
    }

    #[test]
    fn staging_a_file_moves_on_to_the_next_change() {
        let mut p = panel(&[], &["a", "b", "c"], 0);
        p.set_status(Status { staged: entries(&["a"]), unstaged: entries(&["b", "c"]) });
        assert_eq!(selected_path(&p), "b");
    }

    #[test]
    fn selection_follows_a_file_when_the_lists_shift() {
        let mut p = panel(&[], &["a", "b"], 1);
        p.set_status(Status { staged: entries(&["x"]), unstaged: entries(&["a", "b"]) });
        assert_eq!(selected_path(&p), "b");
    }

    #[test]
    fn selection_survives_the_lists_emptying() {
        let mut p = panel(&["a"], &[], 0);
        p.set_status(Status::default());
        assert!(p.selection().is_none());
    }
}
