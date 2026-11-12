//! Feature 4: how two refs differ. The first is "from" and the second "to":
//! a trip through the commit graph, not an order in time. The diff goes from
//! one to the other, with the origin on the left and the destination on the right.
//!
//! The same view shows a stash, opened from `:stash`: its files, and each
//! one's diff against the commit it was made on.

use crate::app::{Action, StashAction};
use crate::file_list::FileList;
use crate::git::{self, DiffSource, Stash};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::thread;

/// What the view shows.
enum Subject {
    Refs {
        from: String,
        to: String,
        /// Commits only in `from`, and only in `to`.
        only_from: usize,
        only_to: usize,
        merge_base: Option<String>,
    },
    Stash(Stash),
}

pub struct ComparePanel {
    subject: Subject,
    files: FileList,
}

impl ComparePanel {
    pub fn new(from: String, to: String) -> Result<ComparePanel> {
        let files = FileList::new(from.clone(), to.clone(), Vec::new());
        let subject = Subject::Refs { from, to, only_from: 0, only_to: 0, merge_base: None };
        let mut compare = ComparePanel { subject, files };
        compare.refresh()?;
        Ok(compare)
    }

    pub fn stash(stash: Stash) -> Result<ComparePanel> {
        let files = FileList::stash(stash.sha.clone(), git::stash_files(&stash)?);
        Ok(ComparePanel { subject: Subject::Stash(stash), files })
    }

    /// For a comparison, three separate git calls, so they run at once.
    pub fn refresh(&mut self) -> Result<()> {
        let Subject::Refs { from, to, only_from, only_to, merge_base } = &mut self.subject else {
            // A stash never changes, though it may be dropped, which actions
            // on it find out.
            return Ok(());
        };
        let (a, b) = (&*from, &*to);
        let (counts, base, files) = thread::scope(|scope| {
            let base = scope.spawn(|| git::merge_base(a, b));
            let files = scope.spawn(|| git::changed_files(a, b));
            let counts = git::ahead_behind(a, b);
            let crashed = || Err(anyhow::anyhow!("git crashed"));
            (counts, base.join().unwrap_or(None), files.join().unwrap_or_else(|_| crashed()))
        });
        (*only_from, *only_to) = counts?;
        *merge_base = base;
        self.files.set_files(files?);
        Ok(())
    }

    /// Swaps from and to. Nothing needs asking git again: the common
    /// ancestor stays, the commit counts trade places, and so do added and
    /// deleted files. An open diff follows, through the action returned.
    pub fn swap(&mut self) -> Action {
        let Subject::Refs { from, to, only_from, only_to, .. } = &mut self.subject else {
            return Action::Message("A stash only goes one way".to_string());
        };
        std::mem::swap(from, to);
        std::mem::swap(only_from, only_to);
        self.files.swap_refs();
        match self.files.selected_source() {
            Some(source) => Action::PreviewDiff(source),
            None => Action::None,
        }
    }

    /// What's compared, as `A → B`, or the stash's name.
    pub fn refs(&self) -> String {
        match &self.subject {
            Subject::Refs { from, to, .. } => format!("{from} → {to}"),
            Subject::Stash(stash) => stash.name.clone(),
        }
    }

    /// The tab's label and the view's title.
    pub fn title(&self) -> String {
        match &self.subject {
            Subject::Refs { .. } => format!("Compare {}", self.refs()),
            Subject::Stash(stash) => format!("Stash {}", stash.name),
        }
    }

    pub fn stash_shown(&self) -> Option<&Stash> {
        match &self.subject {
            Subject::Stash(stash) => Some(stash),
            Subject::Refs { .. } => None,
        }
    }

    pub fn selected_source(&self) -> Option<DiffSource> {
        self.files.selected_source()
    }

    /// The diffs one key away, most likely first: the selected file's, then
    /// for a comparison the same file with from and to swapped, which r leads
    /// to, then the files j, k, g and G lead to.
    pub fn sources_around(&self) -> Vec<DiffSource> {
        let mut sources = self.files.sources_around();
        if let Some(DiffSource::Refs { a, b, path }) = sources.first() {
            let swapped = DiffSource::Refs { a: b.clone(), b: a.clone(), path: path.clone() };
            sources.insert(1, swapped);
        }
        sources
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if let Subject::Stash(stash) = &self.subject {
            let action = match key.code {
                KeyCode::Esc => return Action::CloseStash,
                KeyCode::Char('a') => Some(StashAction::Apply),
                KeyCode::Char('p') => Some(StashAction::Pop),
                KeyCode::Char('d') => Some(StashAction::Drop),
                _ => None,
            };
            if let Some(action) = action {
                return Action::Stash(action, stash.clone());
            }
        } else if key.code == KeyCode::Esc {
            return Action::CloseCompare;
        }
        self.files.handle_key(key)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = ui::panel(format!(" {} ", self.title()), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [summary, list] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(inner);

        let bold = Style::default().add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(ui::DIM);
        let lines = match &self.subject {
            Subject::Refs { from, to, only_from, only_to, merge_base } => {
                let base = match merge_base {
                    Some(sha) => format!("common ancestor {}", &sha[..sha.len().min(7)]),
                    None => "no common ancestor".to_string(),
                };
                // From to's point of view: what it adds to from, and what it lacks.
                vec![
                    Line::from(vec![
                        Span::styled(to.as_str(), bold),
                        Span::raw(" is "),
                        Span::styled(format!("{only_to} ahead"), Style::default().fg(Color::Green)),
                        Span::raw(", "),
                        Span::styled(format!("{only_from} behind"), Style::default().fg(Color::Red)),
                        Span::raw(" "),
                        Span::styled(from.as_str(), bold),
                    ]),
                    Line::styled(base, dim),
                    Line::default(),
                ]
            }
            Subject::Stash(stash) => vec![
                Line::styled(stash.message.as_str(), bold),
                Line::styled(format!("made on {} · {}", &stash.base[..stash.base.len().min(7)], stash.age), dim),
                Line::default(),
            ],
        };
        ui::render_lines(frame, summary, lines);
        self.files.render(frame, list, focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FileEntry;

    fn panel() -> ComparePanel {
        // Already in the order the list sorts files in.
        let files = vec![
            FileEntry { status: 'M', path: "edited".to_string() },
            FileEntry { status: 'D', path: "gone".to_string() },
            FileEntry { status: 'A', path: "new".to_string() },
        ];
        ComparePanel {
            subject: Subject::Refs {
                from: "main".to_string(),
                to: "feature".to_string(),
                only_from: 1,
                only_to: 3,
                merge_base: Some("abc".to_string()),
            },
            files: FileList::new("main".to_string(), "feature".to_string(), files),
        }
    }

    fn counts(compare: &ComparePanel) -> (usize, usize) {
        match &compare.subject {
            Subject::Refs { only_from, only_to, .. } => (*only_from, *only_to),
            Subject::Stash(_) => panic!("not a comparison"),
        }
    }

    #[test]
    fn swapping_mirrors_the_comparison_without_git() {
        let mut compare = panel();
        compare.swap();
        assert_eq!(compare.refs(), "feature → main");
        assert_eq!(counts(&compare), (3, 1));
        let source = compare.selected_source().unwrap();
        assert_eq!(source, DiffSource::Refs { a: "feature".into(), b: "main".into(), path: "edited".into() });
    }

    #[test]
    fn swapping_flips_added_and_deleted_files() {
        let mut compare = panel();
        compare.swap();
        let statuses: Vec<char> = compare.files.statuses();
        assert_eq!(statuses, ['M', 'A', 'D']);
    }

    #[test]
    fn the_swapped_diff_is_prefetched_right_after_the_selected_one() {
        let sources = panel().sources_around();
        assert_eq!(sources[0], DiffSource::Refs { a: "main".into(), b: "feature".into(), path: "edited".into() });
        assert_eq!(sources[1], DiffSource::Refs { a: "feature".into(), b: "main".into(), path: "edited".into() });
    }

    fn stash_panel() -> ComparePanel {
        let stash = Stash {
            sha: "s1".to_string(),
            base: "base".to_string(),
            has_untracked: true,
            name: "stash@{0}".to_string(),
            message: "WIP on main: abc fix".to_string(),
            age: "2 hours ago".to_string(),
        };
        let files = vec![
            FileEntry { status: 'M', path: "edited".to_string() },
            FileEntry { status: '?', path: "new".to_string() },
        ];
        ComparePanel { files: FileList::stash(stash.sha.clone(), files), subject: Subject::Stash(stash) }
    }

    #[test]
    fn a_stash_shows_its_files_as_stashed_changes() {
        let mut stash = stash_panel();
        assert_eq!(stash.title(), "Stash stash@{0}");
        let tracked = DiffSource::Stash { sha: "s1".into(), path: "edited".into(), untracked: false };
        assert_eq!(stash.selected_source(), Some(tracked));
        stash.handle_key(KeyEvent::from(KeyCode::Char('j')));
        let untracked = DiffSource::Stash { sha: "s1".into(), path: "new".into(), untracked: true };
        assert_eq!(stash.selected_source(), Some(untracked));
    }

    #[test]
    fn a_stash_has_its_own_keys_and_doesnt_swap() {
        let mut stash = stash_panel();
        assert!(matches!(stash.handle_key(KeyEvent::from(KeyCode::Char('p'))), Action::Stash(StashAction::Pop, _)));
        assert!(matches!(stash.handle_key(KeyEvent::from(KeyCode::Esc)), Action::CloseStash));
        assert!(matches!(stash.swap(), Action::Message(_)));
    }
}
