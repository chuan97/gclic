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

/// The pane's lists, in the order they're shown.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    /// Only shown while there are conflicts, as in VS Code's "Merge Changes".
    Conflicts,
    Staged,
    Changes,
}

pub struct StagingPanel {
    status: Status,
    /// A merge, rebase or the like that stopped halfway, shown in the title.
    operation: Option<&'static str>,
    /// Index into the conflicted, staged and unstaged files, one after another.
    selected: usize,
    scroll: usize,
}

impl StagingPanel {
    pub fn new() -> StagingPanel {
        StagingPanel { status: Status::default(), operation: None, selected: 0, scroll: 0 }
    }

    pub fn has_staged(&self) -> bool {
        !self.status.staged.is_empty()
    }

    /// Each list with its files, in the order they're shown.
    fn sections(&self) -> [(Section, &Vec<FileEntry>); 3] {
        [
            (Section::Conflicts, &self.status.conflicted),
            (Section::Staged, &self.status.staged),
            (Section::Changes, &self.status.unstaged),
        ]
    }

    fn total(&self) -> usize {
        self.sections().iter().map(|(_, files)| files.len()).sum()
    }

    /// Where a section's files start in the selection's numbering.
    fn offset_of(&self, section: Section) -> usize {
        self.sections().iter().take_while(|(s, _)| *s != section).map(|(_, files)| files.len()).sum()
    }

    fn selection(&self) -> Option<(Section, &FileEntry)> {
        self.entry_at(self.selected)
    }

    /// `index` counts the conflicted files, then the staged, then the unstaged.
    fn entry_at(&self, mut index: usize) -> Option<(Section, &FileEntry)> {
        for (section, files) in self.sections() {
            if index < files.len() {
                return Some((section, &files[index]));
            }
            index -= files.len();
        }
        None
    }

    pub fn selected_source(&self) -> Option<DiffSource> {
        self.source_at(self.selected)
    }

    /// The diffs one key away, most likely first, for prefetching.
    pub fn sources_around(&self) -> Vec<DiffSource> {
        let positions = ui::one_key_away(self.selected, self.total());
        positions.into_iter().filter_map(|index| self.source_at(index)).collect()
    }

    fn source_at(&self, index: usize) -> Option<DiffSource> {
        let (section, entry) = self.entry_at(index)?;
        let path = entry.path.clone();
        Some(match (section, entry.status) {
            (Section::Conflicts, _) => DiffSource::Conflict(path),
            (Section::Staged, _) => DiffSource::Staged(path),
            (Section::Changes, '?') => DiffSource::Untracked(path),
            (Section::Changes, _) => DiffSource::Unstaged(path),
        })
    }

    /// Replaces the file lists, keeping the selection on the same file if it
    /// is still there. If not (it was just staged, say) the selection stays at
    /// the same position in its section, so pressing the stage key repeatedly
    /// walks down the list.
    /// The pane's title, which says when a merge or the like is under way, as
    /// that's when conflicts turn up here.
    pub fn title(&self) -> String {
        match self.operation {
            Some(operation) => format!("{}· {operation} ", TITLE),
            None => TITLE.to_string(),
        }
    }

    pub fn set_status(&mut self, status: Status) {
        // Reading a few file names, so cheap enough for every refresh.
        self.operation = git::operation_in_progress();
        let previous = self
            .selection()
            .map(|(section, entry)| (section, entry.path.clone()));
        let old_offset = previous.as_ref().map_or(0, |(section, _)| self.offset_of(*section));
        self.status = status;

        if let Some((section, path)) = previous {
            let new_offset = self.offset_of(section);
            let list = match section {
                Section::Conflicts => &self.status.conflicted,
                Section::Staged => &self.status.staged,
                Section::Changes => &self.status.unstaged,
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
            KeyCode::Char(' ') => return self.toggle_stage(),
            KeyCode::Char('a') => return Action::after_change(git::stage_all()),
            KeyCode::Char('A') => return Action::after_change(git::unstage_all()),
            KeyCode::Char('d') => return self.ask_to_restore(),
            KeyCode::Char('D') => return self.ask_to_discard_all(),
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
            // Staging a conflicted file is how git marks it resolved.
            Some((Section::Conflicts | Section::Changes, entry)) => Action::after_change(git::stage(&entry.path)),
            None => Action::None,
        }
    }

    /// Whether d can restore the selected file: a change not yet staged, other
    /// than a submodule, which restoring doesn't handle.
    pub fn can_restore(&self) -> bool {
        matches!(self.selection(), Some((Section::Changes, entry)) if !is_folder(&entry.path))
    }

    /// What D would discard: as d does it, file by file, the changed files to
    /// restore and the untracked ones to delete. Submodules are left to git,
    /// and conflicts and staged changes aren't in the list.
    pub fn discardable(&self) -> (Vec<String>, Vec<String>) {
        let files = self.status.unstaged.iter().filter(|entry| !is_folder(&entry.path));
        let (delete, restore): (Vec<&FileEntry>, Vec<&FileEntry>) = files.partition(|entry| entry.status == '?');
        let paths = |entries: Vec<&FileEntry>| entries.into_iter().map(|entry| entry.path.clone()).collect();
        (paths(restore), paths(delete))
    }

    fn ask_to_discard_all(&self) -> Action {
        let (restore, delete) = self.discardable();
        if restore.is_empty() && delete.is_empty() {
            return Action::Message("Nothing to discard: staged changes stay".to_string());
        }
        Action::Confirm(Confirm::DiscardAll { restore, delete })
    }

    fn ask_to_restore(&self) -> Action {
        match self.selection() {
            Some((Section::Staged, _)) => {
                Action::Message("Unstage the file before restoring it".to_string())
            }
            Some((Section::Conflicts, _)) => Action::Message(
                "Resolve the conflict in your editor (o), then stage it (space)".to_string(),
            ),
            // git lists a submodule, or a repo nested inside this one, as a
            // folder. Restoring one isn't a plain restore, so it's left to git.
            Some((Section::Changes, entry)) if is_folder(&entry.path) => Action::Message(format!(
                "{} is a submodule or a nested repo: reset it with git, e.g. git submodule update",
                entry.path.trim_end_matches('/')
            )),
            Some((Section::Changes, entry)) if entry.status == '?' => {
                Action::Confirm(Confirm::DeleteUntracked(entry.path.clone()))
            }
            Some((Section::Changes, entry)) => {
                Action::Confirm(Confirm::Restore(entry.path.clone()))
            }
            None => Action::None,
        }
    }

    /// The rows it asks for: the lists with their headings, or one line if
    /// there's nothing to show.
    pub fn wanted_height(&self) -> u16 {
        match self.total() {
            0 => 1,
            files => (files + self.headings().len()) as u16,
        }
    }

    /// The sections with a heading: Staged and Changes always, and Conflicts
    /// while there are any.
    fn headings(&self) -> Vec<(Section, &Vec<FileEntry>)> {
        let shown = |(section, files): &(Section, &Vec<FileEntry>)| *section != Section::Conflicts || !files.is_empty();
        self.sections().into_iter().filter(shown).collect()
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = ui::panel(self.title(), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.total() == 0 {
            let note = Line::styled("No changes", Style::default().fg(ui::DIM));
            frame.render_widget(note, inner);
            return;
        }

        // The selected file's row on screen, past the headings above it.
        let (mut row, mut index, mut selected_row) = (0, 0, 0);
        for (_, files) in self.headings() {
            row += 1;
            if (index..index + files.len()).contains(&self.selected) {
                selected_row = row + self.selected - index;
                break;
            }
            row += files.len();
            index += files.len();
        }
        self.scroll = ui::keep_visible(self.scroll, selected_row, inner.height as usize);

        let heading = Style::default().fg(ui::DIM).add_modifier(Modifier::BOLD);
        let mut lines = Vec::new();
        let mut index = 0;
        for (section, files) in self.headings() {
            let name = match section {
                Section::Conflicts => "Conflicts",
                Section::Staged => "Staged",
                Section::Changes => "Changes",
            };
            lines.push(Line::styled(format!("{name} ({})", files.len()), heading));
            for entry in files {
                lines.push(file_line(entry, index == self.selected, focused));
                index += 1;
            }
        }
        let lines = lines.into_iter().skip(self.scroll).collect();
        ui::render_lines(frame, inner, lines);
    }
}

fn is_folder(path: &str) -> bool {
    std::path::Path::new(path).is_dir()
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
            status: Status { staged: entries(staged), unstaged: entries(unstaged), ..Status::default() },
            operation: None,
            selected,
            scroll: 0,
        }
    }

    fn selected_path(panel: &StagingPanel) -> &str {
        &panel.selection().unwrap().1.path
    }

    #[test]
    fn discarding_all_restores_changes_and_deletes_new_files_but_not_staged_ones() {
        let mut p = panel(&["staged"], &["edited", "new"], 0);
        p.status.unstaged[1].status = '?';
        p.status.conflicted = conflicts(&["clash"]);
        let (restore, delete) = p.discardable();
        assert_eq!((restore, delete), (vec!["edited".to_string()], vec!["new".to_string()]));
        assert!(matches!(p.handle_key(KeyEvent::from(KeyCode::Char('D'))), Action::Confirm(Confirm::DiscardAll { .. })));
        let nothing = panel(&["staged"], &[], 0);
        assert!(matches!(nothing.ask_to_discard_all(), Action::Message(_)));
    }

    #[test]
    fn staging_a_file_moves_on_to_the_next_change() {
        let mut p = panel(&[], &["a", "b", "c"], 0);
        p.set_status(Status { staged: entries(&["a"]), unstaged: entries(&["b", "c"]), ..Status::default() });
        assert_eq!(selected_path(&p), "b");
    }

    #[test]
    fn selection_follows_a_file_when_the_lists_shift() {
        let mut p = panel(&[], &["a", "b"], 1);
        p.set_status(Status { staged: entries(&["x"]), unstaged: entries(&["a", "b"]), ..Status::default() });
        assert_eq!(selected_path(&p), "b");
    }

    fn conflicts(paths: &[&str]) -> Vec<FileEntry> {
        paths.iter().map(|path| FileEntry { status: 'U', path: path.to_string() }).collect()
    }

    #[test]
    fn conflicts_come_first_and_open_as_conflicts() {
        let mut p = panel(&["s"], &["c"], 0);
        p.status.conflicted = conflicts(&["x"]);
        assert_eq!(p.selected_source(), Some(DiffSource::Conflict("x".to_string())));
        assert_eq!(p.source_at(1), Some(DiffSource::Staged("s".to_string())));
        assert_eq!(p.source_at(2), Some(DiffSource::Unstaged("c".to_string())));
        assert_eq!(p.wanted_height(), 6); // three headings, three files
    }

    #[test]
    fn resolving_a_conflict_moves_on_to_the_next() {
        let mut p = panel(&[], &[], 0);
        p.status.conflicted = conflicts(&["a", "b"]);
        // Staging a marks it resolved, and it joins the staged files.
        p.set_status(Status { conflicted: conflicts(&["b"]), staged: entries(&["a"]), ..Status::default() });
        assert_eq!(selected_path(&p), "b");
    }

    #[test]
    fn without_conflicts_there_is_no_conflicts_heading() {
        assert_eq!(panel(&["a"], &["b"], 0).wanted_height(), 4);
    }

    #[test]
    fn selection_survives_the_lists_emptying() {
        let mut p = panel(&["a"], &[], 0);
        p.set_status(Status::default());
        assert!(p.selection().is_none());
    }
}
