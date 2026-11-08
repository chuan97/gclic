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
        let (ahead, behind) = git::ahead_behind(&a, &b)?;
        let merge_base = git::merge_base(&a, &b);
        let files = FileList::new(a.clone(), b.clone(), git::changed_files(&a, &b)?);
        Ok(ComparePanel { a, b, ahead, behind, merge_base, files })
    }

    pub fn refresh(&mut self) -> Result<()> {
        (self.ahead, self.behind) = git::ahead_behind(&self.a, &self.b)?;
        self.merge_base = git::merge_base(&self.a, &self.b);
        self.files.set_files(git::changed_files(&self.a, &self.b)?);
        Ok(())
    }

    /// Swaps A and B. An open diff follows, through the action returned.
    pub fn swap(&mut self) -> Action {
        std::mem::swap(&mut self.a, &mut self.b);
        self.files.swap_refs();
        if let Err(error) = self.refresh() {
            return Action::Error(error.to_string());
        }
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

    pub fn sources_around(&self) -> Vec<DiffSource> {
        self.files.sources_around()
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
