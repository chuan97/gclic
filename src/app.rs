//! Owns all state, lays out the screen and routes keys. Panels never touch
//! each other: they return an `Action` and `apply` carries it out here.

use crate::compare::ComparePanel;
use crate::details::DetailsPopup;
use crate::diff::DiffPane;
use crate::git::{self, Branch, CommitDetails, DiffSource};
use crate::graph::GraphPanel;
use crate::picker::{Outcome, Picker};
use crate::staging::StagingPanel;
use crate::ui;
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const AUTO_REFRESH: Duration = Duration::from_millis(500);

const HELP: &str = "\
Anywhere
  Tab        next pane             b   check out a branch
  r          refresh               B   choose graph branches
  ?          this help             C   compare two refs
  z          zoom diff             q   quit
  j/k g/G    move                  Enter  open
  Esc        close the diff, or go back

Changes
  Space/s    stage or unstage      a/A  stage/unstage all
  d          restore file          c    commit staged

Graph
  Enter      expand commit or open file diff
  Esc        collapse              y   copy sha
  i          commit details: j/k g/G scroll, y copy sha, i/q/Esc close

Diff
  Ctrl-d/u   half page             n/p  next/previous hunk

Picker
  j/k g/G    move                  i   start typing to filter
  Esc        stop typing, or close
  Space      tick (multi-select)   Enter  confirm";

/// What a panel wants the app to do after handling a key.
pub enum Action {
    None,
    /// Show this diff, opening the pane if needed.
    OpenDiff(DiffSource),
    /// The selection moved: update the diff, but only if one is already open.
    PreviewDiff(DiffSource),
    Refresh,
    Message(String),
    Error(String),
    Confirm(Confirm),
    OpenCommitBox,
    CloseCompare,
    Copy(String),
    ShowDetails(CommitDetails),
}

impl Action {
    /// The outcome of a git command that changed the repository.
    pub fn after_change(result: Result<()>) -> Action {
        match result {
            Ok(()) => Action::Refresh,
            Err(error) => Action::Error(error.to_string()),
        }
    }
}

/// A destructive operation waiting for y/n.
pub enum Confirm {
    Restore(String),
    DeleteUntracked(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Changes,
    Graph,
    Compare,
    Diff,
}

/// What the picker's result will be used for.
enum PickerUse {
    Checkout(Vec<Branch>),
    GraphBranches,
    CompareA,
    CompareB { a: String },
}

/// The popup currently on top of everything, which gets all the keys.
enum Modal {
    None,
    Picker(Picker, PickerUse),
    Confirm(Confirm),
    CommitBox,
    Details(DetailsPopup),
    Error(String),
    Help,
}

pub struct App {
    staging: StagingPanel,
    graph: GraphPanel,
    compare: Option<ComparePanel>,
    diff: Option<DiffPane>,
    zoomed: bool,
    focus: Focus,
    /// The left-hand panel to return to when leaving the diff.
    last_left_focus: Focus,
    modal: Modal,
    commit_draft: String,
    message: String,
    should_quit: bool,
    last_refresh: Instant,
    /// How refs and HEAD looked at the last full reload.
    refs_fingerprint: String,
}

impl App {
    pub fn new() -> App {
        let mut app = App {
            staging: StagingPanel::new(),
            graph: GraphPanel::new(),
            compare: None,
            diff: None,
            zoomed: false,
            focus: Focus::Changes,
            last_left_focus: Focus::Changes,
            modal: Modal::None,
            commit_draft: String::new(),
            message: String::new(),
            should_quit: false,
            last_refresh: Instant::now(),
            refs_fingerprint: String::new(),
        };
        app.refresh();
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press {
                        self.handle_key(key);
                    }
            // Waits while a popup is open: it would only redraw what's behind it.
            let idle = matches!(self.modal, Modal::None);
            if idle && self.last_refresh.elapsed() >= AUTO_REFRESH {
                self.auto_refresh();
            }
        }
        Ok(())
    }

    // --------------------------------------------------------- refreshing

    /// Reloads everything from git, after an action or on `r`.
    fn refresh(&mut self) {
        let result = self.reload_panels();
        self.report(result);
        self.refs_fingerprint = git::refs_fingerprint();
        self.sync_diff();
        self.last_refresh = Instant::now();
    }

    fn reload_panels(&mut self) -> Result<()> {
        self.staging.refresh()?;
        self.graph.refresh()?;
        if let Some(compare) = &mut self.compare {
            compare.refresh()?;
        }
        Ok(())
    }

    /// Picks up what happened outside the app, such as git commands in another
    /// terminal. Runs often, so the graph only reloads when a ref or HEAD moved.
    fn auto_refresh(&mut self) {
        if git::refs_fingerprint() != self.refs_fingerprint {
            self.refresh();
            return;
        }
        let result = self.staging.refresh();
        self.report(result);
        self.sync_diff();
        self.last_refresh = Instant::now();
    }

    /// Brings an open diff up to date. While browsing Changes it follows the
    /// selection, because staging a file moves the selection to another one.
    fn sync_diff(&mut self) {
        if self.diff.is_none() {
            return;
        }
        if self.focus == Focus::Changes {
            match self.staging.selected_source() {
                Some(source) => self.show_diff(source),
                None => self.diff = None,
            }
        } else if let Some(diff) = &mut self.diff {
            let result = diff.reload();
            self.report(result);
        }
    }

    /// Scroll is kept when the diff is the same one, reloaded.
    fn show_diff(&mut self, source: DiffSource) {
        if let Some(diff) = &mut self.diff
            && diff.source == source {
                let result = diff.reload();
                self.report(result);
                return;
            }
        match DiffPane::load(source) {
            Ok(diff) => self.diff = Some(diff),
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    fn report(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.message = error.to_string();
        }
    }

    // ---------------------------------------------------------------- keys

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.message.clear();

        // Taken out of `self` so that handlers can freely use `self` while
        // holding it. Handlers put back whatever should stay open.
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => self.handle_main_key(key),
            Modal::Picker(picker, usage) => self.handle_picker_key(picker, usage, key),
            Modal::Confirm(confirm) => self.handle_confirm_key(confirm, key),
            Modal::CommitBox => self.handle_commit_key(key),
            Modal::Details(popup) => self.handle_details_key(popup, key),
            // Any key dismisses these.
            Modal::Error(_) | Modal::Help => {}
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Tab => self.next_focus(),
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('b') => self.open_checkout_picker(),
            KeyCode::Char('B') => self.open_graph_picker(),
            KeyCode::Char('C') => self.open_compare_picker(),
            KeyCode::Char('z') if self.diff.is_some() => {
                self.zoomed = !self.zoomed;
                if self.zoomed {
                    self.focus_diff();
                }
            }
            KeyCode::Esc => self.handle_escape(),
            _ => self.send_to_focused_panel(key),
        }
    }

    fn handle_escape(&mut self) {
        match self.focus {
            Focus::Diff => {
                self.zoomed = false;
                self.focus = self.last_left_focus;
            }
            _ if self.diff.is_some() => self.diff = None,
            _ => self.send_to_focused_panel(KeyEvent::from(KeyCode::Esc)),
        }
    }

    fn send_to_focused_panel(&mut self, key: KeyEvent) {
        let action = match self.focus {
            Focus::Changes => self.staging.handle_key(key),
            Focus::Graph => self.graph.handle_key(key),
            Focus::Compare => match &mut self.compare {
                Some(compare) => compare.handle_key(key),
                None => Action::None,
            },
            Focus::Diff => {
                if let Some(diff) = &mut self.diff {
                    diff.handle_key(key);
                }
                Action::None
            }
        };
        self.apply(action);
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::OpenDiff(source) => self.show_diff(source),
            Action::PreviewDiff(source) => {
                if self.diff.is_some() {
                    self.show_diff(source);
                }
            }
            Action::Refresh => self.refresh(),
            Action::Message(message) => self.message = message,
            Action::Error(error) => self.modal = Modal::Error(error),
            Action::Confirm(confirm) => self.modal = Modal::Confirm(confirm),
            Action::OpenCommitBox => self.modal = Modal::CommitBox,
            Action::ShowDetails(details) => self.modal = Modal::Details(DetailsPopup::new(details)),
            Action::CloseCompare => {
                self.compare = None;
                self.focus_left(Focus::Changes);
            }
            Action::Copy(sha) => match copy_to_clipboard(&sha) {
                Ok(()) => self.message = format!("Copied {sha}"),
                Err(error) => self.modal = Modal::Error(error.to_string()),
            },
        }
    }

    fn next_focus(&mut self) {
        let mut order = if self.compare.is_some() {
            vec![Focus::Compare]
        } else {
            vec![Focus::Changes, Focus::Graph]
        };
        if self.diff.is_some() {
            order.push(Focus::Diff);
        }
        let current = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = order[(current + 1) % order.len()];
        if next == Focus::Diff {
            self.focus_diff();
        } else {
            self.focus_left(next);
        }
    }

    fn focus_diff(&mut self) {
        if self.focus != Focus::Diff {
            self.last_left_focus = self.focus;
            self.focus = Focus::Diff;
        }
    }

    /// Moves to a left-hand panel, which must be on screen, so zoom ends.
    fn focus_left(&mut self, panel: Focus) {
        self.zoomed = false;
        self.focus = panel;
        self.last_left_focus = panel;
    }

    // -------------------------------------------------------------- modals

    fn handle_confirm_key(&mut self, confirm: Confirm, key: KeyEvent) {
        if key.code == KeyCode::Char('y') {
            let result = match &confirm {
                Confirm::Restore(path) => git::restore(path),
                Confirm::DeleteUntracked(path) => git::delete_untracked(path),
            };
            self.apply(Action::after_change(result));
        }
    }

    fn handle_commit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // Esc keeps the draft for next time.
            KeyCode::Esc => return,
            KeyCode::Char('s') if ctrl => {
                if self.commit_draft.trim().is_empty() {
                    self.message = "The commit message is empty".to_string();
                } else {
                    match git::commit(&self.commit_draft) {
                        Ok(()) => {
                            self.commit_draft.clear();
                            self.refresh();
                            self.message = "Committed".to_string();
                        }
                        // The draft stays, so a failed hook costs no retyping.
                        Err(error) => self.modal = Modal::Error(error.to_string()),
                    }
                    return;
                }
            }
            KeyCode::Enter => self.commit_draft.push('\n'),
            KeyCode::Backspace => {
                self.commit_draft.pop();
            }
            KeyCode::Char(ch) if !ctrl => self.commit_draft.push(ch),
            _ => {}
        }
        self.modal = Modal::CommitBox;
    }

    fn handle_details_key(&mut self, mut popup: DetailsPopup, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('i' | 'q') => {}
            KeyCode::Char('y') => {
                let sha = popup.sha().to_string();
                self.modal = Modal::Details(popup);
                self.apply(Action::Copy(sha));
            }
            _ => {
                popup.handle_key(key);
                self.modal = Modal::Details(popup);
            }
        }
    }

    fn handle_picker_key(&mut self, mut picker: Picker, usage: PickerUse, key: KeyEvent) {
        match picker.handle_key(key) {
            Outcome::Open => self.modal = Modal::Picker(picker, usage),
            Outcome::Cancelled => {}
            Outcome::Picked(picked) => self.use_picked(usage, picked),
        }
    }

    fn use_picked(&mut self, usage: PickerUse, picked: Vec<String>) {
        let first = picked.first().cloned().unwrap_or_default();
        match usage {
            PickerUse::Checkout(branches) => {
                let branch = branches.iter().find(|b| b.name == first);
                let local_name = branch.map_or(first.as_str(), |b| b.local_name.as_str());
                match git::switch(local_name) {
                    Ok(()) => {
                        self.refresh();
                        self.message = format!("On branch {local_name}");
                    }
                    Err(error) => self.modal = Modal::Error(error.to_string()),
                }
            }
            PickerUse::GraphBranches => {
                self.compare = None;
                self.focus_left(Focus::Graph);
                let result = self.graph.set_refs(picked);
                self.report(result);
            }
            PickerUse::CompareA => {
                if git::resolve(&first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                match compare_items() {
                    Ok(items) => {
                        let title = format!("Compare {first} with");
                        let picker = Picker::with_free_text(&title, items);
                        self.modal = Modal::Picker(picker, PickerUse::CompareB { a: first });
                    }
                    Err(error) => self.modal = Modal::Error(error.to_string()),
                }
            }
            PickerUse::CompareB { a } => {
                if git::resolve(&first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                match ComparePanel::new(a, first) {
                    Ok(compare) => {
                        self.compare = Some(compare);
                        self.diff = None;
                        self.focus_left(Focus::Compare);
                    }
                    Err(error) => self.modal = Modal::Error(error.to_string()),
                }
            }
        }
    }

    fn open_checkout_picker(&mut self) {
        match git::branches() {
            Ok(branches) => {
                let names = branches.iter().map(|b| b.name.clone()).collect();
                let picker = Picker::single("Check out branch", names);
                self.modal = Modal::Picker(picker, PickerUse::Checkout(branches));
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    fn open_graph_picker(&mut self) {
        match git::branches() {
            Ok(branches) => {
                let names = branches.into_iter().map(|b| b.name).collect();
                let ticked = self.graph.custom_refs().to_vec();
                let title = "Graph branches: Space ticks, Enter applies (none = default)";
                let picker = Picker::multi(title, names, ticked);
                self.modal = Modal::Picker(picker, PickerUse::GraphBranches);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    fn open_compare_picker(&mut self) {
        match compare_items() {
            Ok(items) => {
                let picker = Picker::with_free_text("Compare ref A", items);
                self.modal = Modal::Picker(picker, PickerUse::CompareA);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    // ------------------------------------------------------------- drawing

    pub fn render(&mut self, frame: &mut Frame) {
        let [main, status] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());

        match (self.diff.is_some(), self.zoomed) {
            (false, _) => self.render_left(frame, main),
            (true, true) => self.render_diff(frame, main),
            (true, false) => {
                let [left, right] =
                    Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                        .areas(main);
                self.render_left(frame, left);
                self.render_diff(frame, right);
            }
        }
        self.render_status_line(frame, status);
        self.render_modal(frame);
    }

    fn render_left(&mut self, frame: &mut Frame, area: Rect) {
        if let Some(compare) = &mut self.compare {
            compare.render(frame, area, self.focus == Focus::Compare);
            return;
        }
        let [top, bottom] =
            Layout::vertical([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(area);
        self.staging.render(frame, top, self.focus == Focus::Changes);
        self.graph.render(frame, bottom, self.focus == Focus::Graph);
    }

    fn render_diff(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Diff;
        if let Some(diff) = &mut self.diff {
            diff.render(frame, area, focused);
        }
    }

    fn render_status_line(&self, frame: &mut Frame, area: Rect) {
        let line = if self.message.is_empty() {
            Line::styled(self.hints(), Style::default().fg(ui::DIM))
        } else {
            Line::styled(self.message.as_str(), Style::default().fg(Color::Yellow))
        };
        frame.render_widget(line, area);
    }

    fn hints(&self) -> &'static str {
        match self.focus {
            Focus::Changes => "space stage · enter diff · d restore · c commit · tab next · ? help · q quit",
            Focus::Graph if self.graph.is_on_file() => "enter diff · esc collapse · j/k move · i details · y copy sha · ? help",
            Focus::Graph => "enter expand · i details · y copy sha · B branches · tab next · ? help · q quit",
            Focus::Compare => "enter diff · esc close · tab next · ? help · q quit",
            Focus::Diff => "j/k scroll · ctrl-d/u page · n/p hunk · z zoom · esc back · ? help",
        }
    }

    fn render_modal(&mut self, frame: &mut Frame) {
        let area = frame.area();
        match &mut self.modal {
            Modal::None => {}
            Modal::Picker(picker, _) => picker.render(frame, area),
            Modal::Details(popup) => popup.render(frame, area),
            Modal::Confirm(confirm) => {
                let text = match confirm {
                    Confirm::Restore(path) => format!("Discard your changes to {path}?"),
                    Confirm::DeleteUntracked(path) => {
                        format!("{path} is untracked. Permanently delete it?")
                    }
                };
                let text = format!("{text}\n\ny = yes · any other key = cancel");
                render_popup(frame, area, " Confirm ", text, (56, 7), Color::Yellow);
            }
            Modal::CommitBox => {
                let text = format!("{}▏", self.commit_draft);
                let title = " Commit message: ctrl-s commits, esc cancels ";
                render_popup(frame, area, title, text, (72, 12), Color::Cyan);
            }
            Modal::Error(error) => {
                let height = (error.lines().count() as u16 + 4).min(20);
                let title = " Git error: any key closes ";
                render_popup(frame, area, title, error.clone(), (72, height), Color::Red);
            }
            Modal::Help => {
                let height = HELP.lines().count() as u16 + 2;
                let title = " Help: any key closes ";
                render_popup(frame, area, title, HELP.to_string(), (68, height), Color::Cyan);
            }
        }
    }
}

fn render_popup(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    text: String,
    (width, height): (u16, u16),
    color: Color,
) {
    let popup = ui::centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = ui::panel(title.to_string(), true).border_style(Style::default().fg(color));
    let paragraph = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn compare_items() -> Result<Vec<String>> {
    let mut items = vec!["HEAD".to_string()];
    items.extend(git::branches()?.into_iter().map(|b| b.name));
    items.extend(git::tags()?);
    Ok(items)
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}
