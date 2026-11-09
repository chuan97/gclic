//! Owns all state, lays out the screen and routes keys. Panels never touch
//! each other: they return an `Action` and `apply` carries it out here.

use crate::branches::BranchesPanel;
use crate::compare::ComparePanel;
use crate::details::DetailsPane;
use crate::diff::{ColorCache, DiffPane, DiffView, Row};
use crate::git::{self, CommitDetails, DiffSource};
use crate::graph::GraphPanel;
use crate::help::HelpPopup;
use crate::picker::{Outcome, Picker};
use crate::staging::StagingPanel;
use crate::ui;
use crate::worker::{Done, Worker};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const AUTO_REFRESH: Duration = Duration::from_millis(500);
/// How long to wait for a key before checking on the worker's results. Short,
/// so colours appear soon after they're ready.
const KEY_WAIT: Duration = Duration::from_millis(30);
const GLOBAL_HINTS: &str = "? help · q quit";
/// The keys shown on the pickers that choose refs to compare.
const COMPARE_PICKER_KEYS: [(&str, &str); 4] =
    [("i", "type"), ("p", "paste"), ("enter", "choose"), ("esc", "close")];
/// The preview's share of the width, focused or not. Zooming gives it all of it.
const DIFF_PERCENT: u16 = 80;
/// The most of the left column's height that Branches and Changes can take,
/// as percentages, so that the graph always keeps a good part of it.
const BRANCHES_MAX_PERCENT: u16 = 40;
const CHANGES_MAX_PERCENT: u16 = 30;
const CHANGES_FOCUSED_MAX_PERCENT: u16 = 50;

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
    /// Compare these two refs, A then B, on the Compare tab.
    Compare(String, String),
    /// Compare this commit with a ref the user picks.
    CompareWith(String),
    Copy(String),
    /// Show these commit details on the right. If they're already showing,
    /// move into them.
    OpenDetails(CommitDetails),
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
    /// Closing loses refs the user may have taken a while to pick, and tab
    /// already leaves the comparison without losing it.
    CloseCompare,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Branches,
    Changes,
    Graph,
    Compare,
    /// The diff or the commit details on the right.
    Preview,
}

/// The two views. Repo is the panes; Compare shows how two refs differ.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Repo,
    Compare,
}

/// What a tab keeps while the other one shows: where the focus was and what
/// was open on the right.
struct TabState {
    focus: Focus,
    last_left_focus: Focus,
    diff: Option<DiffPane>,
    details: Option<DetailsPane>,
    zoomed: bool,
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
    Error(String),
    Help(HelpPopup),
}

pub struct App {
    tab: Tab,
    /// The tab not showing, put aside until `tab` switches back to it.
    other_tab: TabState,
    branches: BranchesPanel,
    staging: StagingPanel,
    graph: GraphPanel,
    compare: Option<ComparePanel>,
    /// The right-hand side shows a diff or a commit's details, never both.
    /// Each stays in its mode: moving onto a commit leaves a diff alone, and
    /// moving onto a file leaves the details alone.
    diff: Option<DiffPane>,
    details: Option<DetailsPane>,
    zoomed: bool,
    focus: Focus,
    /// The left-hand panel to return to when leaving the preview.
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
    /// `zed` commands still running, which hand the file to Zed and exit.
    editor_commands: Vec<Child>,
    /// The background thread doing slow reads: auto-refresh and colouring.
    worker: Worker,
    colors: ColorCache,
    /// Counts the refreshes, which follow every change gclic makes. A poll
    /// asked for before the latest one may have read the repo before a
    /// change, so its result is dropped.
    generation: u64,
}

impl App {
    pub fn new() -> App {
        let mut app = App {
            tab: Tab::Repo,
            other_tab: TabState {
                focus: Focus::Compare,
                last_left_focus: Focus::Compare,
                diff: None,
                details: None,
                zoomed: false,
            },
            branches: BranchesPanel::new(),
            staging: StagingPanel::new(),
            graph: GraphPanel::new(),
            compare: None,
            diff: None,
            details: None,
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
            editor_commands: Vec::new(),
            worker: Worker::start(),
            colors: ColorCache::default(),
            generation: 0,
        };
        app.refresh();
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            self.take_worker_results();
            terminal.draw(|frame| self.render(frame))?;
            if event::poll(KEY_WAIT)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
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

    /// Reloads everything from git, after an action or on `r`. Runs here
    /// rather than on the worker, so a change gclic made shows straight away.
    fn refresh(&mut self) {
        self.generation += 1;
        let result = self.reload_panels();
        self.report(result);
        self.sync_diff();
        self.sync_details();
        self.request_colors();
        self.last_refresh = Instant::now();
    }

    /// The panes' refreshes don't depend on each other, so they run at once,
    /// along with the refs fingerprint: each is a few git calls, and most of
    /// their time is waiting for git. The graph's colours wait for the branch
    /// list, which the Branches pane fetches.
    fn reload_panels(&mut self) -> Result<()> {
        let App { branches, staging, graph, compare, .. } = self;
        let (fingerprint, results) = thread::scope(|scope| {
            let fingerprint = scope.spawn(git::refs_fingerprint);
            let staged = scope.spawn(|| staging.refresh());
            let graphed = scope.spawn(|| graph.refresh());
            let compared = scope.spawn(|| compare.as_mut().map_or(Ok(()), |c| c.refresh()));
            let listed = branches.refresh();
            let crashed = || Err(anyhow::anyhow!("A refresh crashed"));
            let results = [
                listed,
                staged.join().unwrap_or_else(|_| crashed()),
                graphed.join().unwrap_or_else(|_| crashed()),
                compared.join().unwrap_or_else(|_| crashed()),
            ];
            (fingerprint.join().unwrap_or_default(), results)
        });
        self.refs_fingerprint = fingerprint;
        self.graph.color_by(self.branches.list());
        results.into_iter().collect()
    }

    /// Picks up what happened outside the app, such as git commands in another
    /// terminal. The worker does the reading, and `apply_poll` the rest.
    fn auto_refresh(&mut self) {
        let diff = self.diff.as_ref().map(|diff| diff.source.clone());
        self.worker.poll(self.generation, diff);
        // Collects the editor commands that have finished, so none linger.
        self.editor_commands.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        self.last_refresh = Instant::now();
    }

    fn take_worker_results(&mut self) {
        let (results, alive) = self.worker.receive();
        for done in results {
            match done {
                Done::Poll { generation, fingerprint, status, diff } => {
                    self.apply_poll(generation, fingerprint, status, diff);
                }
                Done::Highlight { view, text, rows } => self.apply_highlight(view, text, rows),
            }
        }
        if !alive {
            self.worker = Worker::start();
            self.message = "The background worker stopped, so it was restarted".to_string();
        }
    }

    /// Takes in what a poll found. The graph only reloads, with everything
    /// else, when a ref or HEAD moved.
    fn apply_poll(
        &mut self,
        generation: u64,
        fingerprint: String,
        status: Result<git::Status>,
        diff: Option<(DiffSource, Result<String>)>,
    ) {
        if generation != self.generation {
            return; // A refresh has run since, which read the repo more recently.
        }
        if fingerprint != self.refs_fingerprint {
            self.refresh();
            return;
        }
        match status {
            Ok(status) => self.staging.set_status(status),
            Err(error) => self.message = error.to_string(),
        }
        // While browsing Changes the diff follows the selection, which the new
        // status may have moved. Otherwise it takes the newer text, if any.
        let current = self.diff.as_ref().map(|diff| diff.source.clone());
        let follows = self.focus == Focus::Changes && current.as_ref().is_some_and(|s| s.is_working_tree());
        let selected = self.staging.selected_source();
        if follows && selected != current {
            match selected {
                Some(source) => self.show_diff(source),
                None => self.diff = None,
            }
        } else if let Some(open) = &mut self.diff
            && let Some((source, Ok(text))) = diff
            && open.source == source
        {
            open.set_text(text);
        }
        self.request_colors();
    }

    fn apply_highlight(&mut self, view: DiffView, text: String, rows: Vec<Row>) {
        if let Some(diff) = &mut self.diff
            && diff.source == view.source
            && diff.needs_colors()
        {
            // Only taken if the text matches, so not for the other kind of diff.
            diff.apply_colors(&text, rows.clone());
        }
        self.colors.insert(view, text, rows);
    }

    /// A diff as it would show now, whole or just the changes.
    fn view_of(source: &DiffSource) -> DiffView {
        DiffView { source: source.clone(), whole: git::whole_files() }
    }

    /// Colours the open diff from the cache if it can, and asks the worker for
    /// the rest: the open diff if still uncoloured, the same file the other
    /// way, whole or just the changes, which e leads to, then every diff one
    /// key away in the focused list, so they're ready when the user gets
    /// there. Without a diff open, only the file under the cursor, one enter
    /// away. Asking again replaces what's still waiting.
    fn request_colors(&mut self) {
        let mut wanted = Vec::new();
        if let Some(diff) = &mut self.diff {
            let view = App::view_of(&diff.source);
            if diff.needs_colors() {
                match self.colors.get(&view, diff.text()) {
                    Some(rows) => {
                        let text = diff.text().to_string();
                        diff.apply_colors(&text, rows);
                    }
                    None => wanted.push(view.clone()),
                }
            }
            let other = DiffView { whole: !view.whole, ..view };
            if !self.colors.contains(&other) {
                wanted.push(other);
            }
        }
        let list = if self.focus == Focus::Preview { self.last_left_focus } else { self.focus };
        let mut around = match list {
            Focus::Changes => self.staging.sources_around(),
            Focus::Graph => self.graph.file_sources_around(),
            Focus::Compare => self.compare.as_ref().map(|c| c.sources_around()).unwrap_or_default(),
            Focus::Branches | Focus::Preview => Vec::new(),
        };
        if self.diff.is_none() {
            around.truncate(1);
        }
        for source in around {
            let view = App::view_of(&source);
            if !wanted.contains(&view) && !self.colors.contains(&view) {
                wanted.push(view);
            }
        }
        self.worker.highlight(wanted);
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

    /// Keeps the details on the graph's selected commit, or the commit that
    /// the selected file belongs to.
    fn sync_details(&mut self) {
        let Some(details) = &self.details else {
            return;
        };
        let Some(sha) = self.graph.selected_sha() else {
            self.details = None;
            return;
        };
        if details.sha() != sha {
            match git::commit_details(sha) {
                Ok(details) => self.details = Some(DetailsPane::new(details)),
                Err(error) => self.message = error.to_string(),
            }
        }
    }

    fn has_preview(&self) -> bool {
        self.diff.is_some() || self.details.is_some()
    }

    fn close_preview(&mut self) {
        self.diff = None;
        self.details = None;
    }

    /// Scroll is kept when the diff is the same one, reloaded.
    /// A diff that was prefetched shows straight from the cache, without
    /// waiting for git. If the file has changed since, the next poll or
    /// refresh brings the newer version, as for any change made elsewhere.
    fn show_diff(&mut self, source: DiffSource) {
        if let Some(diff) = &mut self.diff
            && diff.source == source {
                let result = diff.reload();
                self.report(result);
                return;
            }
        if let Some((text, rows)) = self.colors.latest(&App::view_of(&source)) {
            self.diff = Some(DiffPane::colored(source, text, rows));
            self.details = None;
            return;
        }
        match DiffPane::load(source) {
            Ok(diff) => {
                self.diff = Some(diff);
                self.details = None;
            }
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
            Modal::Help(mut help) => {
                if help.handle_key(key) {
                    self.modal = Modal::Help(help);
                }
            }
            // Any key dismisses it.
            Modal::Error(_) => {}
        }
        // The cursor may have moved, so the diffs one key away may have changed.
        self.request_colors();
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
            // h and l move between the Repo tab's panes. Elsewhere they're the
            // pane's own: a diff scrolls sideways, Compare's tree folds.
            KeyCode::Char('h') if self.tab == Tab::Repo && self.focus != Focus::Preview => self.move_focus(false),
            KeyCode::Char('l') if self.tab == Tab::Repo && self.focus != Focus::Preview => self.move_focus(true),
            KeyCode::Char('?') => self.modal = Modal::Help(HelpPopup::new(self.help_section())),
            KeyCode::Char('o') => self.open_in_editor(),
            KeyCode::Char('r') if self.tab == Tab::Compare && self.compare.is_some() => self.swap_compare(),
            KeyCode::Tab | KeyCode::BackTab => self.switch_tab(),
            KeyCode::Char('e') if self.diff.is_some() => self.toggle_whole_files(),
            // On the Compare tab, c starts a new comparison, as c compares from
            // where the user is in the other panes.
            KeyCode::Char('c') if self.focus == Focus::Compare => self.open_compare_picker(),
            KeyCode::Char('z') if self.has_preview() => {
                self.zoomed = !self.zoomed;
                if self.zoomed {
                    self.focus_preview();
                }
            }
            KeyCode::Esc => self.handle_escape(),
            _ => self.send_to_focused_panel(key),
        }
    }

    fn handle_escape(&mut self) {
        match self.focus {
            Focus::Preview => {
                self.zoomed = false;
                self.focus = self.last_left_focus;
                if let Some(diff) = &mut self.diff {
                    diff.reset_shift();
                }
            }
            _ if self.has_preview() => self.close_preview(),
            Focus::Compare if self.compare.is_none() => self.switch_tab(),
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
            Focus::Preview => {
                if let Some(diff) = &mut self.diff {
                    diff.handle_key(key);
                    Action::None
                } else if let Some(details) = &mut self.details {
                    if key.code == KeyCode::Char('y') {
                        Action::Copy(details.sha().to_string())
                    } else {
                        details.handle_key(key);
                        Action::None
                    }
                } else {
                    Action::None
                }
            }
        };
        self.apply(action);
        if self.focus == Focus::Graph {
            self.sync_details();
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::OpenDiff(source) => {
                if self.diff.as_ref().is_some_and(|diff| diff.source == source) {
                    self.focus_preview();
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
            Action::OpenDetails(details) => {
                if self.details.as_ref().is_some_and(|shown| shown.sha() == details.sha) {
                    self.focus_preview();
                } else {
                    self.details = Some(DetailsPane::new(details));
                    self.diff = None;
                }
            }
            // Closing a comparison goes back to the Repo tab, as esc does.
            Action::CloseCompare => self.modal = Modal::Confirm(Confirm::CloseCompare),
            Action::Compare(a, b) => self.start_compare(a, b),
            Action::CompareWith(sha) => self.open_compare_b_picker(git::display_ref(&sha)),
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

    /// Steps along the left-hand panes, wrapping around at the ends. The preview
    /// isn't one of them: Enter or i goes into it and Esc comes back.
    fn move_focus(&mut self, forward: bool) {
        // By the tab showing: a comparison kept on the Compare tab mustn't
        // take over h and l on the Repo tab.
        let order = match self.tab {
            Tab::Compare => vec![Focus::Compare],
            Tab::Repo => vec![Focus::Branches, Focus::Changes, Focus::Graph],
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

    fn focus_preview(&mut self) {
        if self.focus != Focus::Preview {
            self.last_left_focus = self.focus;
            self.focus = Focus::Preview;
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
                    Confirm::CloseCompare => {
                        self.close_compare();
                        return;
                    }
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
            // ctrl-s is kept for terminals that send ctrl-enter as plain enter.
            KeyCode::Enter | KeyCode::Char('s') if ctrl => {
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
                // Confirming the default view's branches as they were keeps the
                // default view, which follows a checkout to another branch.
                let unchanged = self.graph.is_default_view() && same_refs(&picked, &self.graph.shown_refs());
                let refs = if unchanged { Vec::new() } else { picked };
                let result = self.graph.set_refs(refs, self.branches.list());
                self.report(result);
            }
            PickerUse::CompareA => {
                if git::resolve(&first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                self.open_compare_b_picker(git::display_ref(&first));
            }
            PickerUse::CompareB { a } => {
                if git::resolve(&first).is_none() {
                    self.modal = Modal::Error(format!("Unknown ref: {first}"));
                    return;
                }
                self.start_compare(a, git::display_ref(&first));
            }
        }
    }

    /// Asks for the ref to compare `a` with.
    fn open_compare_b_picker(&mut self, a: String) {
        match compare_items(&self.recent_yanks) {
            Ok(mut items) => {
                // Comparing a ref with itself shows nothing.
                items.retain(|item| item.split_whitespace().next() != Some(a.as_str()));
                let title = format!("Compare {a} with: after");
                let picker = Picker::with_free_text(&title, items).with_hints(&COMPARE_PICKER_KEYS);
                self.modal = Modal::Picker(picker, PickerUse::CompareB { a });
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    /// Shows how `a` and `b` differ on the Compare tab, replacing any
    /// comparison already there.
    fn start_compare(&mut self, a: String, b: String) {
        match ComparePanel::new(a, b) {
            Ok(compare) => {
                if self.tab == Tab::Repo {
                    self.switch_tab();
                }
                // Whatever was open on the right belonged to the old comparison.
                self.close_preview();
                self.zoomed = false;
                self.compare = Some(compare);
                self.focus_left(Focus::Compare);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    /// Closing a comparison goes back to the Repo tab, as esc does elsewhere.
    fn close_compare(&mut self) {
        self.compare = None;
        self.close_preview();
        self.switch_tab();
    }

    /// Switches every diff between the whole file and just the changes. It
    /// stays that way while moving between files, for reading with context.
    fn toggle_whole_files(&mut self) {
        git::set_whole_files(!git::whole_files());
        // The cache keeps both kinds, so the other one, prefetched, shows
        // coloured at once.
        if let Some(diff) = &mut self.diff {
            let result = diff.reload();
            self.report(result);
        }
    }

    /// Swaps the showing tab's state with the other tab's.
    fn switch_tab(&mut self) {
        let showing = TabState {
            focus: self.focus,
            last_left_focus: self.last_left_focus,
            diff: self.diff.take(),
            details: self.details.take(),
            zoomed: self.zoomed,
        };
        let other = std::mem::replace(&mut self.other_tab, showing);
        self.focus = other.focus;
        self.last_left_focus = other.last_left_focus;
        self.diff = other.diff;
        self.details = other.details;
        self.zoomed = other.zoomed;
        self.tab = match self.tab {
            Tab::Repo => Tab::Compare,
            Tab::Compare => Tab::Repo,
        };
    }

    fn open_graph_picker(&mut self) {
        match git::branches() {
            Ok(branches) => {
                let names = branches.into_iter().map(|b| b.name).collect();
                // The branches on screen start ticked, the default view's too.
                let ticked = self.graph.shown_refs();
                let hints = [("space", "tick"), ("a/A", "all, none"), ("enter", "apply"), ("esc", "close")];
                let picker = Picker::multi("Graph branches", names, ticked).with_hints(&hints);
                self.modal = Modal::Picker(picker, PickerUse::GraphBranches);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    fn open_compare_picker(&mut self) {
        match compare_items(&self.recent_yanks) {
            Ok(items) => {
                let picker = Picker::with_free_text("Compare: before", items).with_hints(&COMPARE_PICKER_KEYS);
                self.modal = Modal::Picker(picker, PickerUse::CompareA);
            }
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    // ------------------------------------------------------------- drawing

    pub fn render(&mut self, frame: &mut Frame) {
        let [tabs, main, status] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.render_tabs(frame, tabs);

        match (self.has_preview(), self.zoomed) {
            (false, _) => self.render_left(frame, main),
            (true, true) => self.render_preview(frame, main),
            (true, false) => {
                let [left, right] = Layout::horizontal([
                    Constraint::Percentage(100 - DIFF_PERCENT),
                    Constraint::Percentage(DIFF_PERCENT),
                ])
                .areas(main);
                self.render_left(frame, left);
                self.render_preview(frame, right);
            }
        }
        self.render_status_line(frame, status);
        self.render_modal(frame);
    }

    /// The tab bar: both tabs, the showing one highlighted, the comparison's
    /// refs on its tab so it's visible from the Repo tab too. The other tab is
    /// marked with the key that leads to it, like the panes' [h] and [l].
    fn render_tabs(&self, frame: &mut Frame, area: Rect) {
        let area = Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area };
        let compare = match &self.compare {
            Some(compare) => format!(" Compare {} ", compare.refs()),
            None => " Compare ".to_string(),
        };
        let mut spans = Vec::new();
        for (tab, label) in [(Tab::Repo, " Repo ".to_string()), (Tab::Compare, compare)] {
            if tab == self.tab {
                let style = Style::default().fg(Color::Cyan).bg(ui::SELECTED_BG).add_modifier(Modifier::BOLD);
                spans.push(Span::styled(label, style));
            } else {
                spans.push(Span::styled(label, Style::default().fg(ui::DIM)));
                spans.push(Span::styled("[tab]", Style::default().fg(Color::Gray)));
            }
            spans.push(Span::raw("  "));
        }
        frame.render_widget(Line::from(spans), area);
    }

    fn render_left(&mut self, frame: &mut Frame, area: Rect) {
        if self.tab == Tab::Compare {
            let focused = self.focus == Focus::Compare;
            match &mut self.compare {
                Some(compare) => compare.render(frame, area, focused),
                None => render_empty_compare(frame, area, focused),
            }
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
        let changes_title = self.staging.title();
        let panes = [
            (Focus::Branches, branches, crate::branches::TITLE),
            (Focus::Changes, changes, changes_title.as_str()),
            (Focus::Graph, graph, crate::graph::TITLE),
        ];
        if let Some(current) = panes.iter().position(|(pane, ..)| *pane == self.focus) {
            let (_, previous, title) = panes[(current + 2) % 3];
            mark_key(frame, previous, title, "h");
            let (_, next, title) = panes[(current + 1) % 3];
            mark_key(frame, next, title, "l");
        }
    }

    fn render_preview(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == Focus::Preview;
        if let Some(diff) = &mut self.diff {
            diff.render(frame, area, focused);
        } else if let Some(details) = &mut self.details {
            details.render(frame, area, focused);
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
        let (mut spans, empty) = if self.message.is_empty() {
            let hints = self.hints();
            (hint_spans(&hints), hints.is_empty())
        } else {
            (vec![Span::styled(self.message.clone(), Style::default().fg(Color::Yellow))], false)
        };
        // The keys that work everywhere follow, set apart from the pane's own.
        // While typing a branch search, ? and q are typed into it instead.
        if !(self.focus == Focus::Branches && self.branches.is_typing()) {
            if !empty {
                spans.push(Span::styled("   │   ", dim));
            }
            spans.extend(hint_spans(GLOBAL_HINTS));
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
                hints.extend(["enter check out", "c compare", "space remotes", "i search"]);
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
                if self.staging.can_restore() {
                    hints.push("d restore");
                }
                if self.staging.has_staged() {
                    hints.push("c commit");
                }
            }
            Focus::Graph if self.graph.is_on_file() => {
                hints.extend(self.enter_diff_hint(self.graph.selected_file_source()));
                hints.push("n/N commit");
                hints.extend([self.details_hint(), "y copy sha"]);
                esc = Some("esc collapse");
            }
            Focus::Graph => {
                let expanded = self.graph.is_selected_expanded();
                hints.push(if expanded { "enter collapse" } else { "enter expand" });
                hints.extend([self.details_hint(), "c compare", "y copy sha", "b choose branches"]);
                // Only worth it when open commits' files are in the way.
                if self.graph.has_expanded() {
                    hints.push("n/N commit");
                }
                if expanded {
                    esc = Some("esc collapse");
                } else if self.graph.has_expanded() {
                    esc = Some("esc collapse all");
                }
            }
            Focus::Compare if self.compare.is_none() => {
                hints.push("c choose two refs");
                esc = Some("esc back to Repo");
            }
            Focus::Compare => {
                let source = self.compare.as_ref().and_then(|compare| compare.selected_source());
                hints.extend(self.enter_diff_hint(source));
                hints.extend(["h/l fold", "r swap before and after", "c new comparison"]);
                esc = Some("esc close compare");
            }
            Focus::Preview => {
                if self.diff.is_some() {
                    hints.extend(["j/k move", "h/l sideways", "ctrl-d/u page", "n/N change"]);
                    hints.push(if git::whole_files() { "e changes only" } else { "e whole file" });
                } else {
                    hints.extend(["j/k scroll", "ctrl-d/u page", "y copy sha"]);
                }
                hints.push(if self.zoomed { "z unzoom" } else { "z zoom" });
                esc = Some("esc back");
            }
        }
        if self.focused_file().is_some() {
            hints.push("o open in zed");
        }
        if self.focus != Focus::Preview {
            if self.diff.is_some() {
                esc = Some("esc close diff");
            } else if self.details.is_some() {
                esc = Some("esc close details");
            }
        }
        hints.extend(esc);
        hints.join(" · ")
    }

    /// The file under the cursor: the focused diff's, or the one selected in
    /// the focused pane.
    fn focused_file(&self) -> Option<DiffSource> {
        match self.focus {
            Focus::Branches => None,
            Focus::Changes => self.staging.selected_source(),
            Focus::Graph => self.graph.selected_file_source(),
            Focus::Compare => self.compare.as_ref().and_then(|compare| compare.selected_source()),
            Focus::Preview => self.diff.as_ref().map(|diff| diff.source.clone()),
        }
    }

    /// The help section for where the user is, which help marks.
    fn help_section(&self) -> &'static str {
        match self.focus {
            Focus::Branches => "Branches",
            Focus::Changes => "Changes",
            Focus::Graph => "Graph",
            Focus::Compare => "Compare",
            Focus::Preview if self.diff.is_some() => "Diff",
            Focus::Preview => "Commit details",
        }
    }

    /// i shows the selected commit's details, or moves into them if they show.
    fn details_hint(&self) -> &'static str {
        let showing = self.details.as_ref().map(|details| details.sha());
        if showing.is_some() && showing == self.graph.selected_sha() {
            "i focus details"
        } else {
            "i details"
        }
    }

    /// Opens the focused file in Zed, which comes to the front, while gclic
    /// carries on. It opens at the diff's active line, or at the file's first
    /// change without a diff. History opens the file as it is now, so the line
    /// may have moved since.
    fn open_in_editor(&mut self) {
        let Some(source) = self.focused_file() else {
            return;
        };
        let line = match &self.diff {
            Some(diff) if diff.source == source => diff.active_line(),
            _ => DiffPane::load(source.clone()).ok().and_then(|diff| diff.active_line()),
        };
        // gclic runs from the repo root, which paths are relative to.
        let path = match std::env::current_dir() {
            Ok(root) => root.join(source.path()),
            Err(error) => {
                self.modal = Modal::Error(error.to_string());
                return;
            }
        };
        if !path.exists() {
            self.message = format!("{} isn't in the working tree", source.path());
            return;
        }
        let target = format!("{}:{}", path.display(), line.unwrap_or(1));
        let command = Command::new("zed")
            .arg(target)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match command {
            Ok(child) => self.editor_commands.push(child),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.message = "Zed's zed command isn't installed, or isn't on the PATH".to_string();
            }
            Err(error) => self.modal = Modal::Error(format!("Couldn't run zed: {error}")),
        }
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
            Modal::Confirm(confirm) => {
                let bold = Style::default().add_modifier(Modifier::BOLD);
                let (title, body, action) = match confirm {
                    Confirm::Restore(path) => (
                        " Discard changes ",
                        vec![Line::from(vec![
                            Span::raw("Discard your changes to "),
                            Span::styled(path.clone(), bold),
                            Span::raw("?"),
                        ])],
                        "discard",
                    ),
                    Confirm::DeleteUntracked(path) => (
                        " Delete file ",
                        vec![Line::from(vec![
                            Span::styled(path.clone(), bold),
                            Span::raw(" is untracked. Permanently delete it?"),
                        ])],
                        "delete",
                    ),
                    Confirm::CloseCompare => {
                        let refs = self.compare.as_ref().map(|c| c.refs()).unwrap_or_default();
                        let body = vec![
                            Line::from(vec![
                                Span::raw("Close the comparison "),
                                Span::styled(refs, bold),
                                Span::raw("?"),
                            ]),
                            Line::from(vec![
                                Span::styled("tab", Style::default().fg(ui::KEY)),
                                Span::raw(" goes to Repo and keeps it."),
                            ]),
                        ];
                        (" Close comparison ", body, "close")
                    }
                };
                let hints = ui::key_hints(&[("enter", action), ("esc", "cancel")], Some(action));
                // The borders, plus a row in case a long path or ref wraps.
                let height = body.len() as u16 + 3;
                render_popup(frame, area, title, body, hints, (56, height), Color::Cyan);
            }
            Modal::ConfirmQuit => {
                // Quitting loses whatever was open: the graph's branches, the
                // comparison, expanded commits. gclic starts from scratch.
                let hints = ui::key_hints(&[("enter", "quit"), ("esc", "cancel")], Some("quit"));
                let body = vec![Line::raw("Quit gclic?")];
                render_popup(frame, area, " Quit ", body, hints, (40, 4), Color::Cyan);
            }
            Modal::CommitBox => {
                let draft = format!("{}▏", self.commit_draft);
                let body = draft.split('\n').map(|line| Line::raw(line.to_string())).collect();
                let hints = ui::key_hints(&[("ctrl-enter", "commit"), ("esc", "close, keeping it")], None);
                render_popup(frame, area, " Commit message ", body, hints, (72, 12), Color::Cyan);
            }
            Modal::Error(error) => {
                let body: Vec<Line> = error.lines().map(|line| Line::raw(line.to_string())).collect();
                let height = (body.len() as u16 + 3).min(20);
                let hints = ui::key_hints(&[("any key", "closes")], None);
                render_popup(frame, area, " Git error ", body, hints, (72, height), Color::Red);
            }
            Modal::Help(help) => help.render(frame, area),
        }
    }
}

/// Whether two lists hold the same refs, in any order.
fn same_refs(a: &[String], b: &[String]) -> bool {
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    a.sort();
    b.sort();
    a == b
}

/// Hints like `space stage · enter diff` styled as popups show keys: each
/// hint's first word, its key, in the key colour, and the rest in the normal
/// text colour. "type to filter" has no key, so it's all normal.
fn hint_spans(hints: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, hint) in hints.split(" · ").enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(ui::DIM)));
        }
        match hint.split_once(' ') {
            Some((key, action)) if key != "type" => {
                spans.push(Span::styled(key.to_string(), Style::default().fg(ui::KEY)));
                spans.push(Span::raw(format!(" {action}")));
            }
            _ => spans.push(Span::raw(hint.to_string())),
        }
    }
    spans
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

/// The Compare tab before a comparison: how to start one.
fn render_empty_compare(frame: &mut Frame, area: Rect, focused: bool) {
    let block = ui::panel(" Compare ".to_string(), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let line = Line::from(vec![
        Span::raw("No comparison yet. "),
        Span::styled("c", Style::default().fg(ui::KEY)),
        Span::raw(" starts one."),
    ]);
    frame.render_widget(line, inner);
}

/// A popup in the middle of the screen: its title says what it is, and its
/// bottom border lists the keys, as `ui::key_hints` makes them. It's wide
/// enough for those keys, even if `width` is less. The border is cyan, so the
/// keys' yellow and a dangerous action's red stand out from it, except for an
/// error's, which is red.
fn render_popup(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    body: Vec<Line<'static>>,
    hints: Line<'static>,
    (width, height): (u16, u16),
    color: Color,
) {
    let width = width.max(hints.width() as u16 + 4);
    let popup = ui::centered(area, width, height);
    frame.render_widget(Clear, popup);
    let block = ui::panel(title.to_string(), true)
        .border_style(Style::default().fg(color))
        .title_bottom(hints);
    let paragraph = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
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

/// The first word on the clipboard, which is all a ref could be. None if it
/// holds no text.
fn read_clipboard() -> Result<Option<String>> {
    match arboard::Clipboard::new()?.get_text() {
        Ok(text) => Ok(text.split_whitespace().next().map(str::to_string)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// On Linux the copy may only last while gclic runs, as there the program
/// that copied holds the clipboard. macOS and Windows keep it.
fn copy_to_clipboard(text: &str) -> Result<()> {
    arboard::Clipboard::new()?.set_text(text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These build a whole app on gclic's own repo.

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    #[test]
    fn each_tab_keeps_its_own_focus() {
        let mut app = App::new();
        app.focus_left(Focus::Graph);
        press(&mut app, KeyCode::Tab);
        assert!(app.tab == Tab::Compare && app.focus == Focus::Compare);
        press(&mut app, KeyCode::Tab);
        assert!(app.tab == Tab::Repo && app.focus == Focus::Graph);
    }

    #[test]
    fn h_and_l_still_move_on_the_repo_tab_with_a_comparison_kept() {
        let mut app = App::new();
        app.start_compare("HEAD~1".to_string(), "HEAD".to_string());
        press(&mut app, KeyCode::Tab);
        assert!(app.tab == Tab::Repo && app.compare.is_some());
        app.focus_left(Focus::Changes);
        press(&mut app, KeyCode::Char('l'));
        assert!(app.focus == Focus::Graph);
        press(&mut app, KeyCode::Char('h'));
        assert!(app.focus == Focus::Changes);
    }

    #[test]
    fn hints_show_keys_in_the_key_colour() {
        let spans = hint_spans("space stage · a/A stage/unstage all · type to filter");
        let text: Vec<(&str, Option<Color>)> = spans.iter().map(|s| (s.content.as_ref(), s.style.fg)).collect();
        assert_eq!(
            text,
            [
                ("space", Some(ui::KEY)),
                (" stage", None),
                (" · ", Some(ui::DIM)),
                ("a/A", Some(ui::KEY)),
                (" stage/unstage all", None),
                (" · ", Some(ui::DIM)),
                ("type to filter", None),
            ]
        );
    }

    #[test]
    fn confirming_the_default_branches_keeps_the_default_view() {
        let mut app = App::new();
        let shown = app.graph.shown_refs();
        assert!(app.graph.is_default_view() && !shown.is_empty());
        app.use_picked(PickerUse::GraphBranches, shown.into_iter().rev().collect());
        assert!(app.graph.is_default_view());
        // Picking something else makes a custom view.
        app.use_picked(PickerUse::GraphBranches, vec!["HEAD~1".to_string()]);
        assert!(!app.graph.is_default_view());
    }

    #[test]
    fn esc_on_an_empty_compare_tab_goes_back_to_repo() {
        let mut app = App::new();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Esc);
        assert!(app.tab == Tab::Repo);
    }

    #[test]
    fn c_on_the_compare_tab_asks_for_refs() {
        let mut app = App::new();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('c'));
        assert!(matches!(app.modal, Modal::Picker(_, PickerUse::CompareA)));
    }

    #[test]
    fn starting_a_comparison_switches_to_its_tab() {
        let mut app = App::new();
        app.start_compare("HEAD~1".to_string(), "HEAD".to_string());
        assert!(app.tab == Tab::Compare && app.focus == Focus::Compare);
        assert!(app.compare.as_ref().is_some_and(|c| c.refs() == "HEAD~1 → HEAD"));
        // Closing it asks first, and esc cancels.
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.modal, Modal::Confirm(Confirm::CloseCompare)));
        press(&mut app, KeyCode::Esc);
        assert!(app.tab == Tab::Compare && app.compare.is_some());
        // Enter closes it, going back to where the user was.
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter);
        assert!(app.tab == Tab::Repo && app.compare.is_none());
    }

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
