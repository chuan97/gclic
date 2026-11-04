//! Owns all state, lays out the screen and routes keys. Panels never touch
//! each other: they return an `Action` and `apply` carries it out here.

use crate::branches::BranchesPanel;
use crate::compare::ComparePanel;
use crate::details::DetailsPopup;
use crate::diff::DiffPane;
use crate::git::{self, CommitDetails, DiffSource};
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
  h/l        previous/next pane    C   compare two refs
  ?          this help
  z          zoom diff             q   quit
  j/k g/G    move                  Enter  open
  Esc        close the diff, or go back

Branches
  Enter      check out             i   start typing to filter
  Space      show a branch's remotes

Changes
  Space/s    stage or unstage      a/A  stage/unstage all
  d          restore file          c    commit staged

Graph
  Enter      expand commit or open file diff
  b          choose which branches to show
  Esc        collapse              y   copy sha
  i          commit details: j/k g/G scroll, y copy sha, i/Esc close

Compare
  r          swap A and B, also from its diff

Diff
  Ctrl-d/u   half page             n/p  next/previous hunk

Picker
  j/k g/G    move                  i   start typing to filter
  Esc        stop typing, or close
  Space      tick (multi-select)   a/A  tick/untick all shown
  Enter      confirm (nothing ticked means the default view)
  p          paste a ref from the clipboard, when comparing";

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
    OpenGraphPicker,
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
    Branches,
    Changes,
    Graph,
    Compare,
    Diff,
}

/// What the picker's result will be used for.
enum PickerUse {
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
    ConfirmQuit,
    Details(DetailsPopup),
    Error(String),
    Help,
}

pub struct App {
    branches: BranchesPanel,
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
    /// The last commits yanked, oldest first, as labels starting with the sha.
    recent_yanks: Vec<String>,
    message: String,
    should_quit: bool,
    last_refresh: Instant,
    /// How refs and HEAD looked at the last full reload.
    refs_fingerprint: String,
}

impl App {
    pub fn new() -> App {
        let mut app = App {
            branches: BranchesPanel::new(),
            staging: StagingPanel::new(),
            graph: GraphPanel::new(),
            compare: None,
            diff: None,
            zoomed: false,
            focus: Focus::Changes,
            last_left_focus: Focus::Changes,
            modal: Modal::None,
            commit_draft: String::new(),
            recent_yanks: Vec::new(),
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
        self.branches.refresh()?;
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
        let shows_working_tree = self.diff.as_ref().is_some_and(|diff| diff.source.is_working_tree());
        if self.focus == Focus::Changes && shows_working_tree {
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
            Modal::ConfirmQuit => match key.code {
                KeyCode::Enter => self.should_quit = true,
                KeyCode::Esc => {}
                _ => self.modal = Modal::ConfirmQuit,
            },
            Modal::Details(popup) => self.handle_details_key(popup, key),
            // Any key dismisses these.
            Modal::Error(_) | Modal::Help => {}
        }
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            // While searching branches, letters belong to the search.
            _ if self.focus == Focus::Branches && self.branches.is_typing() => {
                self.send_to_focused_panel(key);
            }
            KeyCode::Char('q') => self.modal = Modal::ConfirmQuit,
            KeyCode::Char('h') => self.move_focus(false),
            KeyCode::Char('l') => self.move_focus(true),
            KeyCode::Char('?') => self.modal = Modal::Help,
            KeyCode::Char('r') if self.compare.is_some() => self.swap_compare(),
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
            Focus::Branches => self.branches.handle_key(key),
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
            Action::OpenGraphPicker => self.open_graph_picker(),
            Action::ShowDetails(details) => self.modal = Modal::Details(DetailsPopup::new(details)),
            Action::CloseCompare => {
                self.compare = None;
                self.focus_left(Focus::Changes);
            }
            Action::Copy(sha) => match copy_to_clipboard(&sha) {
                Ok(()) => {
                    if let Some(label) = git::commit_label(&sha) {
                        remember_yank(&mut self.recent_yanks, label);
                    }
                    self.message = format!("Copied {sha}");
                }
                Err(error) => self.modal = Modal::Error(error.to_string()),
            },
        }
    }

    fn swap_compare(&mut self) {
        if let Some(compare) = &mut self.compare {
            let action = compare.swap();
            self.apply(action);
        }
    }

    /// Steps along the panes from left to right, wrapping around at the ends.
    fn move_focus(&mut self, forward: bool) {
        let mut order = if self.compare.is_some() {
            vec![Focus::Compare]
        } else {
            vec![Focus::Branches, Focus::Changes, Focus::Graph]
        };
        if self.diff.is_some() {
            order.push(Focus::Diff);
        }
        let current = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward {
            ui::next_index(current, order.len())
        } else {
            ui::previous_index(current, order.len())
        };
        if order[next] == Focus::Diff {
            self.focus_diff();
        } else if next != current {
            self.focus_left(order[next]);
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
            KeyCode::Esc | KeyCode::Char('i') => {}
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
        // `p` pastes a ref, so a sha yanked in the graph is two keys from comparing.
        let paste = key.code == KeyCode::Char('p') && key.modifiers == KeyModifiers::NONE;
        if paste && picker.accepts_free_text() && !picker.is_typing() {
            match read_clipboard() {
                Ok(Some(text)) => picker.set_query(text),
                Ok(None) => self.message = "The clipboard is empty".to_string(),
                Err(error) => {
                    self.modal = Modal::Error(error.to_string());
                    return;
                }
            }
            self.modal = Modal::Picker(picker, usage);
            return;
        }
        match picker.handle_key(key) {
            Outcome::Open => self.modal = Modal::Picker(picker, usage),
            Outcome::Cancelled => {}
            Outcome::Picked(picked) => self.use_picked(usage, picked),
        }
    }

    fn use_picked(&mut self, usage: PickerUse, picked: Vec<String>) {
        // Refs have no spaces, so this also takes the sha out of a yank's label.
        let first = picked.first().and_then(|item| item.split_whitespace().next());
        let first = first.unwrap_or_default().to_string();
        match usage {
            PickerUse::GraphBranches => {
                let result = self.graph.set_refs(picked);
                self.report(result);
            }
            PickerUse::CompareA => {
                if git::resolve(&first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                let a = git::display_ref(&first);
                match compare_items(&self.recent_yanks) {
                    Ok(mut items) => {
                        // Comparing a ref with itself shows nothing.
                        items.retain(|item| item.split_whitespace().next() != Some(a.as_str()));
                        let title = format!("Compare {a} with: i types, p pastes");
                        let picker = Picker::with_free_text(&title, items);
                        self.modal = Modal::Picker(picker, PickerUse::CompareB { a });
                    }
                    Err(error) => self.modal = Modal::Error(error.to_string()),
                }
            }
            PickerUse::CompareB { a } => {
                if git::resolve(&first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                match ComparePanel::new(a, git::display_ref(&first)) {
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

    fn open_graph_picker(&mut self) {
        match git::branches() {
            Ok(branches) => {
                let names = branches.into_iter().map(|b| b.name).collect();
                let ticked = self.graph.custom_refs().to_vec();
                let title = "Graph branches: space ticks, a/A all, enter applies";
                let picker = Picker::multi(title, names, ticked);
                self.modal = Modal::Picker(picker, PickerUse::GraphBranches);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    fn open_compare_picker(&mut self) {
        match compare_items(&self.recent_yanks) {
            Ok(items) => {
                let picker = Picker::with_free_text("Compare ref A: i types, p pastes", items);
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
        let [branches, changes, graph] = Layout::vertical([
            Constraint::Percentage(20),
            Constraint::Percentage(30),
            Constraint::Percentage(50),
        ])
        .areas(area);
        self.branches.render(frame, branches, self.focus == Focus::Branches);
        self.staging.render(frame, changes, self.focus == Focus::Changes);
        self.graph.render(frame, graph, self.focus == Focus::Graph);
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
            Focus::Branches => "enter check out · space remotes · i search · esc clear search · h/l panes · ? help · q quit",
            Focus::Changes => "space stage · enter diff · d restore · c commit · h/l panes · ? help · q quit",
            Focus::Graph if self.graph.is_on_file() => "enter diff · esc collapse · j/k move · i details · y copy sha · ? help",
            Focus::Graph => "enter expand · i details · y copy sha · b choose branches · h/l panes · ? help · q quit",
            Focus::Compare => "enter diff · r swap A and B · esc close · h/l panes · ? help · q quit",
            Focus::Diff => "j/k scroll · ctrl-d/u page · n/p hunk · z zoom · h panes · esc back · ? help",
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
            Modal::ConfirmQuit => {
                let text = "Quit mygc?\n\nenter = quit · esc = cancel".to_string();
                render_popup(frame, area, " Quit ", text, (40, 5), Color::Yellow);
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

/// Keeps the last two yanks, the most a comparison can use. In yank order, so
/// that the first one yanked is the top choice for A and the second for B.
fn remember_yank(yanks: &mut Vec<String>, label: String) {
    let sha = label.split_whitespace().next().unwrap_or_default();
    yanks.retain(|yank| yank.split_whitespace().next() != Some(sha));
    yanks.push(label);
    if yanks.len() > 2 {
        yanks.remove(0);
    }
}

fn compare_items(recent_yanks: &[String]) -> Result<Vec<String>> {
    let mut items = recent_yanks.to_vec();
    items.push("HEAD".to_string());
    items.extend(git::branches()?.into_iter().map(|b| b.name));
    items.extend(git::tags()?);
    Ok(items)
}

/// The first word on the clipboard, which is all a ref could be. None if empty.
fn read_clipboard() -> Result<Option<String>> {
    let output = Command::new("pbpaste").stdin(Stdio::null()).output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.split_whitespace().next().map(str::to_string))
}

fn copy_to_clipboard(text: &str) -> Result<()> {
    let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yanks(labels: &[&str]) -> Vec<String> {
        let mut yanks = Vec::new();
        for label in labels {
            remember_yank(&mut yanks, label.to_string());
        }
        yanks
    }

    #[test]
    fn yanks_keep_their_order() {
        assert_eq!(yanks(&["aaa  one", "bbb  two"]), ["aaa  one", "bbb  two"]);
    }

    #[test]
    fn only_the_last_two_yanks_are_kept() {
        assert_eq!(yanks(&["aaa  one", "bbb  two", "ccc  three"]), ["bbb  two", "ccc  three"]);
    }

    #[test]
    fn yanking_a_commit_again_moves_it_to_the_end() {
        assert_eq!(yanks(&["aaa  one", "bbb  two", "aaa  one"]), ["bbb  two", "aaa  one"]);
    }
}
