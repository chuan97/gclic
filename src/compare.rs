//! Feature 4: how two refs differ.

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
    a: String,
    b: String,
    ahead: usize,
    behind: usize,
    merge_base: Option<String>,
    files: FileList,
}

impl ComparePanel {
    pub fn new(a: String, b: String) -> Result<ComparePanel> {
        let files = FileList::new(a.clone(), b.clone(), Vec::new());
        let mut compare = ComparePanel { a, b, ahead: 0, behind: 0, merge_base: None, files };
        compare.refresh()?;
        Ok(compare)
    }

    /// Three separate git calls, so they run at once.
    pub fn refresh(&mut self) -> Result<()> {
        let (a, b) = (&self.a, &self.b);
        let (counts, merge_base, files) = thread::scope(|scope| {
            let merge_base = scope.spawn(|| git::merge_base(a, b));
            let files = scope.spawn(|| git::changed_files(a, b));
            let counts = git::ahead_behind(a, b);
            let crashed = || Err(anyhow::anyhow!("git crashed"));
            (counts, merge_base.join().unwrap_or(None), files.join().unwrap_or_else(|_| crashed()))
        });
        (self.ahead, self.behind) = counts?;
        self.merge_base = merge_base;
        self.files.set_files(files?);
        Ok(())
    }

    /// Swaps A and B. Nothing needs asking git again: the common ancestor
    /// stays, ahead and behind trade places, and so do added and deleted
    /// files. An open diff follows, through the action returned.
    pub fn swap(&mut self) -> Action {
        std::mem::swap(&mut self.a, &mut self.b);
        std::mem::swap(&mut self.ahead, &mut self.behind);
        self.files.swap_refs();
        match self.files.selected_source() {
            Some(source) => Action::PreviewDiff(source),
            None => Action::None,
        }
    }

    /// What's compared, as `A → B`.
    pub fn refs(&self) -> String {
        format!("{} → {}", self.a, self.b)
    }

    pub fn selected_source(&self) -> Option<DiffSource> {
        self.files.selected_source()
    }

    /// The diffs one key away, most likely first: the selected file's, then
    /// the same file with A and B swapped, which r leads to, then the files
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
        let block = ui::panel(format!(" Compare {} → {} ", self.a, self.b), focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let [summary, list] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(inner);

        let bold = Style::default().add_modifier(Modifier::BOLD);
        let base = match &self.merge_base {
            Some(sha) => format!("common ancestor {}", &sha[..sha.len().min(7)]),
            None => "no common ancestor".to_string(),
        };
        let lines = vec![
            Line::from(vec![
                Span::styled(self.a.as_str(), bold),
                Span::raw(" is "),
                Span::styled(format!("{} ahead", self.ahead), Style::default().fg(Color::Green)),
                Span::raw(", "),
                Span::styled(format!("{} behind", self.behind), Style::default().fg(Color::Red)),
                Span::raw(" "),
                Span::styled(self.b.as_str(), bold),
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
        let files = vec![
            FileEntry { status: 'A', path: "new".to_string() },
            FileEntry { status: 'D', path: "gone".to_string() },
            FileEntry { status: 'M', path: "edited".to_string() },
        ];
        ComparePanel {
            a: "main".to_string(),
            b: "feature".to_string(),
            ahead: 1,
            behind: 3,
            merge_base: Some("abc".to_string()),
            files: FileList::new("main".to_string(), "feature".to_string(), files),
        }
    }

    #[test]
    fn swapping_mirrors_the_comparison_without_git() {
        let mut compare = panel();
        compare.swap();
        assert_eq!(compare.refs(), "feature → main");
        assert_eq!((compare.ahead, compare.behind), (3, 1));
        assert_eq!(compare.merge_base.as_deref(), Some("abc"));
        let source = compare.selected_source().unwrap();
        assert_eq!(source, DiffSource::Refs { a: "feature".into(), b: "main".into(), path: "new".into() });
    }

    #[test]
    fn swapping_flips_added_and_deleted_files() {
        let mut compare = panel();
        compare.swap();
        let statuses: Vec<char> = compare.files.statuses();
        assert_eq!(statuses, ['D', 'A', 'M']);
    }

    #[test]
    fn the_swapped_diff_is_prefetched_right_after_the_selected_one() {
        let sources = panel().sources_around();
        assert_eq!(sources[0], DiffSource::Refs { a: "main".into(), b: "feature".into(), path: "new".into() });
        assert_eq!(sources[1], DiffSource::Refs { a: "feature".into(), b: "main".into(), path: "new".into() });
    }
}
