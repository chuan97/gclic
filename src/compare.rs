//! Feature 4: how two refs differ. The first is "before" and the second
//! "after": the diff goes from one to the other, as VS Code puts the
//! original on the left and the change on the right.

use crate::app::Action;
use crate::file_list::FileList;
use crate::git::{self, DiffSource};
use crate::ui;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::thread;

pub struct ComparePanel {
    before: String,
    after: String,
    /// Commits only in `before`, and only in `after`.
    only_before: usize,
    only_after: usize,
    merge_base: Option<String>,
    files: FileList,
}

impl ComparePanel {
    pub fn new(before: String, after: String) -> Result<ComparePanel> {
        let files = FileList::new(before.clone(), after.clone(), Vec::new());
        let mut compare =
            ComparePanel { before, after, only_before: 0, only_after: 0, merge_base: None, files };
        compare.refresh()?;
        Ok(compare)
    }

    /// Three separate git calls, so they run at once.
    pub fn refresh(&mut self) -> Result<()> {
        let (a, b) = (&self.before, &self.after);
        let (counts, merge_base, files) = thread::scope(|scope| {
            let merge_base = scope.spawn(|| git::merge_base(a, b));
            let files = scope.spawn(|| git::changed_files(a, b));
            let counts = git::ahead_behind(a, b);
            let crashed = || Err(anyhow::anyhow!("git crashed"));
            (counts, merge_base.join().unwrap_or(None), files.join().unwrap_or_else(|_| crashed()))
        });
        (self.only_before, self.only_after) = counts?;
        self.merge_base = merge_base;
        self.files.set_files(files?);
        Ok(())
    }

    /// Swaps before and after. Nothing needs asking git again: the common
    /// ancestor stays, the commit counts trade places, and so do added and
    /// deleted files. An open diff follows, through the action returned.
    pub fn swap(&mut self) -> Action {
        std::mem::swap(&mut self.before, &mut self.after);
        std::mem::swap(&mut self.only_before, &mut self.only_after);
        self.files.swap_refs();
        match self.files.selected_source() {
            Some(source) => Action::PreviewDiff(source),
            None => Action::None,
        }
    }

    /// What's compared, as `A → B`.
    pub fn refs(&self) -> String {
        format!("{} → {}", self.before, self.after)
    }

    pub fn selected_source(&self) -> Option<DiffSource> {
        self.files.selected_source()
    }

    /// The diffs one key away, most likely first: the selected file's, then
    /// the same file with before and after swapped, which r leads to, then the files
    /// j, k, g and G lead to.
    pub fn sources_around(&self) -> Vec<DiffSource> {
        let mut sources = self.files.sources_around();
        if let Some(DiffSource::Refs { a, b, path }) = sources.first() {
            let swapped = DiffSource::Refs { a: b.clone(), b: a.clone(), path: path.clone() };
            sources.insert(1, swapped);
        }
        sources
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            return Action::CloseCompare;
        }
        self.files.handle_key(key)
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = ui::panel(format!(" Compare {} ", self.refs()), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [summary, list] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(inner);

        let bold = Style::default().add_modifier(Modifier::BOLD);
        let base = match &self.merge_base {
            Some(sha) => format!("common ancestor {}", &sha[..sha.len().min(7)]),
            None => "no common ancestor".to_string(),
        };
        // From after's point of view: what it adds to before, and what it lacks.
        let lines = vec![
            Line::from(vec![
                Span::styled(self.after.as_str(), bold),
                Span::raw(" is "),
                Span::styled(format!("{} ahead", self.only_after), Style::default().fg(Color::Green)),
                Span::raw(", "),
                Span::styled(format!("{} behind", self.only_before), Style::default().fg(Color::Red)),
                Span::raw(" "),
                Span::styled(self.before.as_str(), bold),
            ]),
            Line::styled(base, Style::default().fg(ui::DIM)),
            Line::default(),
        ];
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
            before: "main".to_string(),
            after: "feature".to_string(),
            only_before: 1,
            only_after: 3,
            merge_base: Some("abc".to_string()),
            files: FileList::new("main".to_string(), "feature".to_string(), files),
        }
    }

    #[test]
    fn swapping_mirrors_the_comparison_without_git() {
        let mut compare = panel();
        compare.swap();
        assert_eq!(compare.refs(), "feature → main");
        assert_eq!((compare.only_before, compare.only_after), (3, 1));
        assert_eq!(compare.merge_base.as_deref(), Some("abc"));
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
}
