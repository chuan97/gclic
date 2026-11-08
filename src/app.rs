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
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const AUTO_REFRESH: Duration = Duration::from_millis(500);
const GLOBAL_HINTS: &str = "? help · q quit";
/// The diff's share of the width, focused or not. Zooming gives it all of it.
const DIFF_PERCENT: u16 = 80;
/// The most of the left column's height that Branches and Changes can take,
/// as percentages, so that the graph always keeps a good part of it.
const BRANCHES_MAX_PERCENT: u16 = 40;
const CHANGES_MAX_PERCENT: u16 = 30;
const CHANGES_FOCUSED_MAX_PERCENT: u16 = 50;

const HELP: &str = "\
Anywhere
  h/l        previous/next pane    C   compare two refs
  ?          this help
  z          zoom diff             q   quit
  j/k g/G    move                  Enter  open
  Enter      on a file: show its diff, then again to move into it
  Esc        leave the diff, then again to close it

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
  h/l        scroll sideways

Picker
  j/k g/G    move                  i   start typing to filter
  Esc        stop typing, or close
  Space      tick (multi-select)   a/A  tick/untick all shown
  Enter      confirm (nothing ticked means the default view)
  p          paste a ref from the clipboard, when comparing";

/// What a panel wants the app to do after handling a key.
pub enum Action {
    None,
    /// Show this diff, opening the pane if needed. If it's already showing,
    /// move into it.
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
            // Inside the diff, h and l scroll it sideways.
            KeyCode::Char('h') if self.focus != Focus::Diff => self.move_focus(false),
            KeyCode::Char('l') if self.focus != Focus::Diff => self.move_focus(true),
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
                if let Some(diff) = &mut self.diff {
                    diff.reset_shift();
                }
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
            Action::OpenDiff(source) => {
                if self.diff.as_ref().is_some_and(|diff| diff.source == source) {
                    self.focus_diff();
                } else {
                    self.show_diff(source);
                }
            }
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

    /// Steps along the left-hand panes, wrapping around at the ends. The diff
    /// isn't one of them: Enter goes into it and Esc comes back.
    fn move_focus(&mut self, forward: bool) {
        let order = if self.compare.is_some() {
            vec![Focus::Compare]
        } else {
            vec![Focus::Branches, Focus::Changes, Focus::Graph]
        };
        let current = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward {
            ui::next_index(current, order.len())
        } else {
            ui::previous_index(current, order.len())
        };
        if next != current {
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

    /// Like quitting: enter goes ahead, esc cancels, and other keys are ignored
    /// so that a stray one can't do either.
    fn handle_confirm_key(&mut self, confirm: Confirm, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let result = match &confirm {
                    Confirm::Restore(path) => git::restore(path),
                    Confirm::DeleteUntracked(path) => git::delete_untracked(path),
                };
                self.apply(Action::after_change(result));
            }
            KeyCode::Esc => {}
            _ => self.modal = Modal::Confirm(confirm),
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
                let [left, right] = Layout::horizontal([
                    Constraint::Percentage(100 - DIFF_PERCENT),
                    Constraint::Percentage(DIFF_PERCENT),
                ])
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
        // Branches and Changes take the rows they ask for, up to a share of the
        // height, and the graph gets the rest. Borders add two rows to each.
        let fit = |rows: u16, max_percent: u16| (rows + 2).min(area.height * max_percent / 100).max(3);
        let branches_height = fit(
            self.branches.wanted_height(self.focus == Focus::Branches),
            BRANCHES_MAX_PERCENT,
        );
        let changes_max = if self.focus == Focus::Changes {
            CHANGES_FOCUSED_MAX_PERCENT
        } else {
            CHANGES_MAX_PERCENT
        };
        let changes_height = fit(self.staging.wanted_height(), changes_max);
        let [branches, changes, graph] = Layout::vertical([
            Constraint::Length(branches_height),
            Constraint::Length(changes_height),
            Constraint::Min(0),
        ])
        .areas(area);
        self.branches.render(frame, branches, self.focus == Focus::Branches);
        self.staging.render(frame, changes, self.focus == Focus::Changes);
        self.graph.render(frame, graph, self.focus == Focus::Graph);

        // The panes that h and l lead to say so next to their title, in place
        // of a hint. With three panes and wrapping, those are the two not focused.
        let panes = [
            (Focus::Branches, branches, crate::branches::TITLE),
            (Focus::Changes, changes, crate::staging::TITLE),
            (Focus::Graph, graph, crate::graph::TITLE),
        ];
        if let Some(current) = panes.iter().position(|(pane, ..)| *pane == self.focus) {
            let (_, previous, title) = panes[(current + 2) % 3];
            mark_key(frame, previous, title, "h");
            let (_, next, title) = panes[(current + 1) % 3];
            mark_key(frame, next, title, "l");
        }
    }

    fn render_diff(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Diff;
        if let Some(diff) = &mut self.diff {
            diff.render(frame, area, focused);
        }
    }

    /// The pane's own keys, then the keys that work everywhere.
    fn render_status_line(&self, frame: &mut Frame, area: Rect) {
        // A popup takes every key and lists its own on its border.
        if !matches!(self.modal, Modal::None) {
            return;
        }
        // A column of margin at each end, which also lines the text up with
        // the text inside the panes' borders.
        let area = Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area };
        let dim = Style::default().fg(ui::DIM);
        let mut spans = if self.message.is_empty() {
            vec![Span::styled(self.hints(), dim)]
        } else {
            vec![Span::styled(self.message.as_str(), Style::default().fg(Color::Yellow))]
        };
        // The keys that work everywhere follow, set apart from the pane's own.
        // While typing a branch search, ? and q are typed into it instead.
        if !(self.focus == Focus::Branches && self.branches.is_typing()) {
            if !spans[0].content.is_empty() {
                spans.push(Span::styled("   │   ", dim));
            }
            spans.push(Span::styled(GLOBAL_HINTS, dim));
        }
        frame.render_widget(Line::from(spans), area);
    }

    /// The keys that do something right now in the focused pane, so the line
    /// only lists what applies.
    fn hints(&self) -> String {
        let mut hints: Vec<&str> = Vec::new();
        // What esc does in the focused pane, unless an open diff takes it.
        let mut esc = None;
        match self.focus {
            Focus::Branches if self.branches.is_typing() => {
                // Every other key is typed into the search, ? and q included.
                return "type to filter · enter check out · esc stop typing".to_string();
            }
            Focus::Branches => {
                hints.extend(["enter check out", "space remotes", "i search"]);
                if self.branches.is_searching() {
                    esc = Some("esc clear search");
                }
            }
            Focus::Changes => {
                let source = self.staging.selected_source();
                match &source {
                    Some(DiffSource::Staged(_)) => hints.push("space unstage"),
                    Some(_) => hints.push("space stage"),
                    None => {}
                }
                if source.is_some() {
                    hints.push("a/A stage/unstage all");
                }
                hints.extend(self.enter_diff_hint(source.clone()));
                if matches!(source, Some(DiffSource::Unstaged(_) | DiffSource::Untracked(_))) {
                    hints.push("d restore");
                }
                if self.staging.has_staged() {
                    hints.push("c commit");
                }
            }
            Focus::Graph if self.graph.is_on_file() => {
                hints.extend(self.enter_diff_hint(self.graph.selected_file_source()));
                hints.extend(["i details", "y copy sha"]);
                esc = Some("esc collapse");
            }
            Focus::Graph => {
                let expanded = self.graph.is_selected_expanded();
                hints.push(if expanded { "enter collapse" } else { "enter expand" });
                hints.extend(["i details", "y copy sha", "b choose branches"]);
                if expanded {
                    esc = Some("esc collapse");
                } else if self.graph.has_expanded() {
                    esc = Some("esc collapse all");
                }
            }
            Focus::Compare => {
                let source = self.compare.as_ref().and_then(|compare| compare.selected_source());
                hints.extend(self.enter_diff_hint(source));
                hints.push("r swap A and B");
                esc = Some("esc close compare");
            }
            Focus::Diff => {
                hints.extend(["j/k scroll", "h/l sideways", "ctrl-d/u page", "n/p hunk"]);
                hints.push(if self.zoomed { "z unzoom" } else { "z zoom" });
                esc = Some("esc back");
            }
        }
        if self.diff.is_some() && self.focus != Focus::Diff {
            esc = Some("esc close diff");
        }
        hints.extend(esc);
        hints.join(" · ")
    }

    /// Enter on a file shows its diff, or moves into the diff if it's showing.
    fn enter_diff_hint(&self, source: Option<DiffSource>) -> Option<&'static str> {
        let source = source?;
        let showing = self.diff.as_ref().is_some_and(|diff| diff.source == source);
        Some(if showing { "enter focus diff" } else { "enter diff" })
    }

    fn render_modal(&mut self, frame: &mut Frame) {
        let area = frame.area();
        match &mut self.modal {
            Modal::None => {}
            Modal::Picker(picker, _) => picker.render(frame, area),
            Modal::Details(popup) => popup.render(frame, area),
            Modal::Confirm(confirm) => {
                let text = match confirm {
                    Confirm::Restore(path) => {
                        format!("Discard your changes to {path}?\n\nenter = discard · esc = cancel")
                    }
                    Confirm::DeleteUntracked(path) => format!(
                        "{path} is untracked. Permanently delete it?\n\nenter = delete · esc = cancel"
                    ),
                };
                render_popup(frame, area, " Confirm ", text, (56, 7), Color::Yellow);
            }
            Modal::ConfirmQuit => {
                let text = "Quit gclic?\n\nenter = quit · esc = cancel".to_string();
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

/// Writes `[key]` on a pane's top border, right after its title.
fn mark_key(frame: &mut Frame, pane: Rect, title: &str, key: &str) {
    // The trailing space keeps it apart from the border line, like the title.
    let label = format!("[{key}] ");
    // One column for the corner, and one more to leave before the other corner.
    let start = 1 + title.chars().count() as u16;
    let width = label.len() as u16;
    if pane.height == 0 || start + width + 1 > pane.width {
        return;
    }
    let area = Rect { x: pane.x + start, y: pane.y, width, height: 1 };
    frame.render_widget(Line::styled(label, Style::default().fg(Color::Gray)), area);
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
