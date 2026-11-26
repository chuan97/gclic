//! Owns all state and routes keys. Panels never touch each other: they
//! return an `Action` and `apply` carries it out here. The modules below
//! draw the screen, refresh, run commands and handle popups and comments.

mod changes;
mod commands;
mod comments;
mod fetcher;
mod layout;
mod popups;
mod refresh;
mod status_line;
#[cfg(test)]
mod tests;

pub(crate) use commands::COMMAND_HELP;
use commands::read_clipboard;
use comments::CommentBox;
use fetcher::Fetcher;
use layout::project_location;
use popups::{Modal, PickerUse, remember_yank};

use crate::accent::{self, Accent};
use crate::branches::BranchesPanel;
use crate::compare::ComparePanel;
use crate::details::DetailsPane;
use crate::diff::{self, ColorCache, Colored, DiffPane, DiffView};
use crate::editor::{self, Launch};
use crate::git::{self, CommitDetails, DiffSource, Repo, Stash};
use crate::graph::GraphPanel;
use crate::help::HelpPopup;
use crate::picker::{Outcome, Picker};
use crate::repos;
use crate::review::{self, Anchor, Comment, ListAction, Review, ReviewList};
use crate::running::{OutputPopup, Running};
use crate::staging::StagingPanel;
use crate::ui;
use crate::undo;
use crate::worker::{Done, Worker};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How often the repo is read again for changes made outside gclic, such
/// as in an editor or another terminal: often while it's in use, and rarely
/// once no key has been pressed for a while, as each time starts a few git
/// processes. The first key after that reads it again at once.
const REFRESH_IN_USE: Duration = Duration::from_secs(1);
const REFRESH_IDLE: Duration = Duration::from_secs(10);
const IDLE_AFTER: Duration = Duration::from_secs(30);
/// How often to check whether Neovim moved to another repo, which only
/// reads a file's date.
const FOLLOW_EVERY: Duration = Duration::from_millis(500);
/// How long to wait for a key before checking on the worker's results. Short,
/// so colours appear soon after they're ready.
const KEY_WAIT: Duration = Duration::from_millis(30);
const GLOBAL_HINTS: [ui::Hint; 3] = [(":", "command"), ("?", "help"), ("q", "quit")];
/// The keys shown on `:stash`'s list.
const STASH_PICKER_KEYS: [(&str, &str); 6] = [
    ("enter", "files"),
    ("a", "apply"),
    ("p", "pop"),
    ("d", "drop"),
    ("i", "search"),
    ("esc", "close"),
];
/// The keys shown on the pickers that choose refs to compare.
const COMPARE_PICKER_KEYS: [(&str, &str); 4] = [
    ("i", "type"),
    ("p", "paste"),
    ("enter", "choose"),
    ("esc", "close"),
];
/// The preview's share of the width, focused or not. Zooming gives it all of it.
const DIFF_PERCENT: u16 = 80;
const REVIEW_DIFF_PERCENT: u16 = 60;
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
    /// Do this with a stash, from the stash view or the graph.
    Stash(StashAction, Stash),
    /// Leave the stash view, back to the list of stashes.
    CloseStash,
    /// Stage or unstage files, from the Changes pane, which the app does so
    /// it can be undone.
    Staging(Staging),
}

/// What the Changes pane stages or unstages.
pub enum Staging {
    Stage(String),
    Unstage(String),
    StageAll,
    UnstageAll,
}

/// What can be done with a stash. Making one is left to `:git stash`, which
/// has the options for what to put in it.
#[derive(Clone, Copy, PartialEq)]
pub enum StashAction {
    /// Its changes back, all unstaged, keeping it.
    Apply,
    /// Apply, then drop it, unless applying conflicted.
    Pop,
    Drop,
}

impl StashAction {
    /// The stash keys, the same in the graph, the stash view and `:stash`.
    pub fn from_key(key: KeyCode) -> Option<StashAction> {
        match key {
            KeyCode::Char('a') => Some(StashAction::Apply),
            KeyCode::Char('p') => Some(StashAction::Pop),
            KeyCode::Char('d') => Some(StashAction::Drop),
            _ => None,
        }
    }
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
    /// d inside a file's diff: the change the active line is in, as a patch
    /// to take back out of the file.
    RestoreChange {
        path: String,
        patch: String,
    },
    DeleteUntracked(String),
    /// D: every change not staged, as d would discard each.
    DiscardAll {
        restore: Vec<String>,
        delete: Vec<String>,
    },
    /// Closing loses refs the user may have taken a while to pick, and tab
    /// already leaves the comparison without losing it.
    CloseCompare,
    DropStash(Stash),
    /// d on the Review tab: this comment, by its index in the review.
    DeleteComment(usize),
    /// D on the Review tab: every comment.
    ClearReview,
}

/// The panes on the left. Each tab has its own: Branches, Changes and Graph
/// on the Repo tab, and one each on the others.
#[derive(Clone, Copy, PartialEq)]
enum Pane {
    Branches,
    Changes,
    Graph,
    Compare,
    /// The Review tab's list of comments.
    Review,
}

/// The views. Repo is the panes; Compare shows how two refs differ; Review
/// lists the comments written on diffs, and only shows while there are some.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Repo,
    Compare,
    Review,
}

/// The right-hand side: a diff or a commit's details, never both. Each stays
/// in its mode: moving onto a commit leaves a diff alone, and moving onto a
/// file leaves the details alone.
enum Preview {
    None,
    Diff(DiffPane),
    Details(DetailsPane),
}

/// What a tab shows, which it keeps while another tab shows.
struct View {
    /// The pane focused on the left, or the one esc goes back to from the
    /// preview.
    pane: Pane,
    /// Whether the user moved into the preview.
    in_preview: bool,
    preview: Preview,
    zoomed: bool,
}

impl View {
    /// A tab as it first opens: on its own list, with nothing on the right.
    fn fresh(tab: Tab) -> View {
        let pane = match tab {
            Tab::Repo => Pane::Changes,
            Tab::Compare => Pane::Compare,
            Tab::Review => Pane::Review,
        };
        View {
            pane,
            in_preview: false,
            preview: Preview::None,
            zoomed: false,
        }
    }

    /// Whether this pane has the focus, rather than another or the preview.
    fn focused(&self, pane: Pane) -> bool {
        !self.in_preview && self.pane == pane
    }

    fn diff(&self) -> Option<&DiffPane> {
        match &self.preview {
            Preview::Diff(diff) => Some(diff),
            _ => None,
        }
    }

    fn diff_mut(&mut self) -> Option<&mut DiffPane> {
        match &mut self.preview {
            Preview::Diff(diff) => Some(diff),
            _ => None,
        }
    }

    fn details(&self) -> Option<&DetailsPane> {
        match &self.preview {
            Preview::Details(details) => Some(details),
            _ => None,
        }
    }

    fn has_preview(&self) -> bool {
        !matches!(self.preview, Preview::None)
    }

    /// Closes the diff, leaving any details open.
    fn close_diff(&mut self) {
        if matches!(self.preview, Preview::Diff(_)) {
            self.preview = Preview::None;
        }
    }
}

pub struct App {
    /// The repo shown, which every git call runs in.
    repo: Repo,
    /// What `u` undoes: the stagings and discards made in this repo.
    history: undo::History,
    tab: Tab,
    /// The showing tab's focus and preview.
    view: View,
    /// The tabs not showing, by `Tab`, each put aside until it shows again.
    /// None for a tab that opens fresh.
    saved_views: [Option<View>; 3],
    branches: BranchesPanel,
    staging: StagingPanel,
    graph: GraphPanel,
    compare: Option<ComparePanel>,
    /// Comments on diffs' lines, for whoever made the changes.
    review: Review,
    review_list: ReviewList,
    /// Diffs side by side or unified, once the user chooses with `s`. Until
    /// then it depends on the room there is.
    split: Option<bool>,
    /// Diffs show the whole file rather than just the changes, after `e`. It
    /// stays that way while moving between files, for reading with context.
    whole: bool,
    modal: Modal,
    /// `g` was pressed, and the next key says what for: `gg` the top, `gc`
    /// and `gC` a comment.
    pending_g: bool,
    commit_draft: String,
    /// The last commits yanked, oldest first, as labels starting with the sha.
    recent_yanks: Vec<String>,
    message: String,
    should_quit: bool,
    last_refresh: Instant,
    /// How refs and HEAD looked at the last full reload.
    refs_fingerprint: String,
    /// Editors, and messages to Neovim, started in the background and still
    /// running, collected once they end.
    editor_commands: Vec<Child>,
    /// The background thread doing slow reads: auto-refresh and colouring.
    worker: Worker,
    colors: ColorCache,
    /// A `:git` command running in the background.
    running: Option<Running>,
    /// The command to run next, after one that failed: what git suggested,
    /// or the same one in the terminal. The command line offers it, and tab
    /// takes it. Gone once any command runs.
    suggestion: Option<String>,
    fetcher: Fetcher,
    /// A `:!` command to run once the key that asked for it is handled, as
    /// it needs the terminal, which only the main loop has.
    shell_line: Option<String>,
    /// A terminal editor to open, for the same reason: what to show as its
    /// command, its name, and the command.
    editor_in_terminal: Option<(String, String, Command)>,
    /// Inside Neovim, how to follow its working folder to another repo.
    follow: Option<editor::Follow>,
    /// What `o` does, "open in nvim", naming the editor it opens.
    editor_hint: String,
    /// Where `:cd` looks for repos. It stays put as the repo changes, so the
    /// same repos are a few keys away.
    search_root: PathBuf,
    /// Where the repo is, for the tab bar: the folder it's in, with `~` for
    /// home, and its own name.
    project: (String, String),
    /// The repo's colour, for the bar at the top.
    accent: Accent,
    /// Whether that bar shows, which `:color off` turns off for every repo.
    accent_bar: bool,
    /// Counts the refreshes, which follow every change gclic makes. A poll
    /// asked for before the latest one may have read the repo before a
    /// change, so its result is dropped.
    generation: u64,
}

impl App {
    pub fn new(repo: Repo, search_root: PathBuf) -> App {
        let mut app = App {
            search_root,
            running: None,
            suggestion: None,
            fetcher: Fetcher::new(&repo),
            shell_line: None,
            editor_in_terminal: None,
            follow: editor::Follow::from_env(),
            editor_hint: match editor::parent_nvim() {
                Some(_) => "open in nvim".to_string(),
                None => format!("open in {}", editor::name(&editor::setting())),
            },
            tab: Tab::Repo,
            view: View::fresh(Tab::Repo),
            saved_views: [None, None, None],
            review: Review::default(),
            review_list: ReviewList::new(),
            branches: BranchesPanel::new(&repo),
            staging: StagingPanel::new(&repo),
            graph: GraphPanel::new(&repo),
            compare: None,
            split: None,
            whole: false,
            modal: Modal::None,
            pending_g: false,
            commit_draft: String::new(),
            recent_yanks: Vec::new(),
            message: String::new(),
            should_quit: false,
            last_refresh: Instant::now(),
            refs_fingerprint: String::new(),
            editor_commands: Vec::new(),
            worker: Worker::start(&repo),
            colors: ColorCache::default(),
            project: project_location(repo.root()),
            accent: Accent::of(&repo),
            accent_bar: accent::bar_shown(&repo),
            generation: 0,
            history: undo::History::default(),
            repo,
        };
        app.refresh();
        app
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        // gclic only draws after a key, a resize, or news from the worker, a
        // command or a fetch, and when the fetch note's "10s ago" moves on.
        let mut changed = true;
        let mut fetch_note = None;
        let mut last_key = Instant::now();
        let mut followed = Instant::now();
        while !self.should_quit {
            changed |= self.take_worker_results();
            changed |= self.take_command_result();
            changed |= self.fetcher.keep_fetching();
            self.branches.search_sha();
            let note = self.fetcher.note();
            changed |= note != fetch_note;
            if changed {
                terminal.draw(|frame| self.render(frame))?;
                changed = false;
                fetch_note = note;
            }
            if event::poll(KEY_WAIT)? {
                let was_idle = last_key.elapsed() >= IDLE_AFTER;
                self.handle_waiting_keys(terminal)?;
                changed = true;
                last_key = Instant::now();
                // Back after a while away: what changed meanwhile shows now.
                if was_idle {
                    self.poll_repo();
                }
            }
            if followed.elapsed() >= FOLLOW_EVERY {
                changed |= self.follow_neovim();
                followed = Instant::now();
            }
            // Popups included, as they only cover part of the screen.
            if self.last_refresh.elapsed() >= refresh_every(last_key.elapsed()) {
                self.poll_repo();
            }
        }
        Ok(())
    }

    /// Handles every key already waiting before the next draw. A paste comes
    /// as one key per letter, so it then shows at once, not letter by letter.
    fn handle_waiting_keys(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        loop {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key);
                if let Some(line) = self.shell_line.take() {
                    let note = crate::terminal::run_in_terminal(terminal, self.repo.root(), &line)?;
                    self.refresh();
                    if let Some(note) = note {
                        self.message = note;
                    }
                }
                if let Some((shown, name, command)) = self.editor_in_terminal.take() {
                    let note =
                        crate::terminal::hand_terminal(terminal, &shown, &name, vec![command])?;
                    // The file may have changed.
                    self.refresh();
                    if let Some(note) = note {
                        self.message = note;
                    }
                }
            }
            if self.should_quit || !event::poll(Duration::ZERO)? {
                return Ok(());
            }
        }
    }

    fn close_preview(&mut self) {
        self.view.preview = Preview::None;
    }

    /// Scroll is kept when the diff is the same one, reloaded.
    /// A diff that was prefetched shows straight from the cache, without
    /// waiting for git. If the file has changed since, the next poll or
    /// refresh brings the newer version, as for any change made elsewhere.
    fn show_diff(&mut self, source: DiffSource) {
        if let Some(diff) = self.view.diff_mut()
            && diff.source == source
        {
            let result = diff.reload(&self.repo);
            self.report(result);
            return;
        }
        let view = self.view_of(&source);
        if let Some((text, colored)) = self.colors.latest(&view) {
            self.view.preview = Preview::Diff(DiffPane::colored(&self.repo, view, text, colored));
            return;
        }
        match DiffPane::load(&self.repo, view) {
            Ok(diff) => self.view.preview = Preview::Diff(diff),
            // A refresh can get here while a popup is open, such as the
            // command line while typing, which an error popup would replace.
            Err(error) if !matches!(self.modal, Modal::None) => self.message = error.to_string(),
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    fn report(&mut self, result: Result<()>) {
        if let Err(error) = result {
            self.message = error.to_string();
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.message.clear();
        // gg goes to the top in the popups too, as in the panes, where
        // handle_main_key waits for the second g.
        let takes_gg = matches!(&self.modal, Modal::Help(_) | Modal::Output(_))
            || matches!(&self.modal, Modal::Picker(picker, _) if !picker.is_typing());
        let key = if takes_gg {
            ui::gg(&mut self.pending_g, key)
        } else {
            Some(key)
        };
        let Some(key) = key else {
            return;
        };

        // Taken out of `self` so that handlers can freely use `self` while
        // holding it. Handlers put back whatever should stay open.
        match std::mem::replace(&mut self.modal, Modal::None) {
            Modal::None => self.handle_main_key(key),
            Modal::Picker(picker, usage) => self.handle_picker_key(picker, usage, key),
            Modal::Confirm(confirm) => self.handle_confirm_key(confirm, key),
            Modal::CommitBox => self.handle_commit_key(key),
            Modal::CommentBox(comment) => self.handle_comment_key(comment, key),
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
            Modal::Command(line) => self.handle_command_key(line, key),
            Modal::Output(mut output) => {
                if output.handle_key(key) {
                    self.modal = Modal::Output(output);
                }
            }
        }
        // The cursor may have moved, so the diffs one key away may have changed.
        self.request_colors();
    }

    fn handle_main_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // While a command runs, ctrl-c stops it rather than gclic.
            KeyCode::Char('c') if ctrl && self.running.is_some() => {
                if let Some(running) = &mut self.running {
                    running.stop();
                }
            }
            // Asks first, as q does: pressed to stop a command just as it ends,
            // it would otherwise quit gclic.
            KeyCode::Char('c') if ctrl => self.modal = Modal::ConfirmQuit,
            // While searching branches, letters belong to the search.
            _ if self.view.focused(Pane::Branches) && self.branches.is_typing() => {
                self.send_to_focused_panel(key);
            }
            // The key after g.
            _ if self.pending_g => {
                self.pending_g = false;
                match key.code {
                    KeyCode::Char('g') => {
                        self.send_to_focused_panel(KeyEvent::from(KeyCode::Char('g')))
                    }
                    KeyCode::Char('c') => self.comment_on_line(),
                    KeyCode::Char('C') => self.comment_on_file(),
                    _ => {}
                }
            }
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('u') if key.modifiers.is_empty() && self.where_staging_is() => {
                self.undo(false)
            }
            KeyCode::Char('r')
                if key.modifiers == KeyModifiers::CONTROL && self.where_staging_is() =>
            {
                self.undo(true)
            }
            KeyCode::Char('q') => self.modal = Modal::ConfirmQuit,
            // h and l move between the Repo tab's panes. Elsewhere they're the
            // pane's own: a diff scrolls sideways, Compare's tree folds.
            KeyCode::Char('h') if self.tab == Tab::Repo && !self.view.in_preview => {
                self.move_focus(false)
            }
            KeyCode::Char('l') if self.tab == Tab::Repo && !self.view.in_preview => {
                self.move_focus(true)
            }
            KeyCode::Char('?') => self.modal = Modal::Help(HelpPopup::new(self.help_section())),
            KeyCode::Char(':') => self.modal = Modal::Command(String::new()),
            KeyCode::Char('o') => self.open_in_editor(),
            KeyCode::Char('r') if self.tab == Tab::Compare && self.compare.is_some() => {
                self.swap_compare()
            }
            KeyCode::Tab => self.cycle_tab(true),
            KeyCode::BackTab => self.cycle_tab(false),
            KeyCode::Char('e') if self.view.diff().is_some() => self.toggle_whole_files(),
            // A diff on screen takes n and N from any pane, so its changes can
            // be stepped through while browsing files. The graph's own n and N,
            // between commits, work when there's no diff.
            KeyCode::Char('n' | 'N') if !self.view.in_preview && self.view.diff().is_some() => {
                if let Some(diff) = self.view.diff_mut() {
                    diff.handle_key(key);
                }
            }
            // Inside a working-tree diff, space takes the change the active
            // line is in, as it takes the file in the list.
            KeyCode::Char(' ') if self.in_working_tree_diff() => {
                self.stage_active_change();
            }
            KeyCode::Char('d') if key.modifiers.is_empty() && self.in_working_tree_diff() => {
                self.ask_to_restore_active_change();
            }
            KeyCode::Char('s') if self.view.diff().is_some() => {
                self.split = self.view.diff().map(|diff| !diff.is_split());
            }
            // On the Compare tab, c starts a new comparison, as c compares from
            // where the user is in the other panes.
            KeyCode::Char('c') if self.view.focused(Pane::Compare) => self.open_compare_picker(),
            KeyCode::Char('z') if self.view.has_preview() => {
                self.view.zoomed = !self.view.zoomed;
                if self.view.zoomed {
                    self.focus_preview();
                }
            }
            KeyCode::Esc => self.handle_escape(),
            _ => self.send_to_focused_panel(key),
        }
    }

    fn handle_escape(&mut self) {
        if self.view.in_preview {
            self.view.zoomed = false;
            self.view.in_preview = false;
            if let Some(diff) = self.view.diff_mut() {
                diff.reset_shift();
            }
        } else if self.view.has_preview() {
            self.close_preview();
        } else if self.view.pane == Pane::Compare && self.compare.is_none() {
            self.go_to_tab(Tab::Repo);
        } else {
            self.send_to_focused_panel(KeyEvent::from(KeyCode::Esc));
        }
    }

    fn send_to_focused_panel(&mut self, key: KeyEvent) {
        let action = if self.view.in_preview {
            match &mut self.view.preview {
                Preview::Diff(diff) => {
                    diff.handle_key(key);
                    Action::None
                }
                Preview::Details(details) if key.code == KeyCode::Char('y') => {
                    Action::Copy(details.sha().to_string())
                }
                Preview::Details(details) => {
                    details.handle_key(key);
                    Action::None
                }
                Preview::None => Action::None,
            }
        } else {
            match self.view.pane {
                Pane::Branches => self.branches.handle_key(key),
                Pane::Changes => self.staging.handle_key(key),
                Pane::Graph => self.graph.handle_key(key),
                Pane::Compare => match &mut self.compare {
                    Some(compare) => compare.handle_key(key),
                    None => Action::None,
                },
                Pane::Review => {
                    let action = self.review_list.handle_key(&self.review, key);
                    self.act_on_review(action);
                    Action::None
                }
            }
        };
        self.apply(action);
        if self.view.focused(Pane::Graph) {
            self.sync_details();
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::OpenDiff(source) => {
                if self.view.diff().is_some_and(|diff| diff.source == source) {
                    self.focus_preview();
                } else {
                    self.show_diff(source);
                }
            }
            Action::PreviewDiff(source) => {
                if self.view.diff().is_some() {
                    self.show_diff(source);
                }
            }
            Action::Refresh => self.refresh(),
            Action::Staging(staging) => self.stage(staging),
            Action::Message(message) => self.message = message,
            Action::Error(error) => self.modal = Modal::Error(error),
            Action::Confirm(confirm) => self.modal = Modal::Confirm(confirm),
            Action::OpenCommitBox => self.modal = Modal::CommitBox,
            Action::OpenGraphPicker => self.open_graph_picker(),
            Action::OpenDetails(details) => {
                if self
                    .view
                    .details()
                    .is_some_and(|shown| shown.sha() == details.sha)
                {
                    self.focus_preview();
                } else {
                    self.view.preview = Preview::Details(DetailsPane::new(details));
                }
            }
            // Closing a comparison goes back to the Repo tab, as esc does.
            Action::CloseCompare => self.modal = Modal::Confirm(Confirm::CloseCompare),
            Action::Stash(action, stash) => self.act_on_stash(action, stash),
            Action::CloseStash => {
                self.close_compare();
                self.open_stash_picker();
            }
            Action::Compare(a, b) => self.start_compare(a, b),
            Action::CompareWith(sha) => {
                self.open_compare_b_picker(git::display_ref(&self.repo, &sha))
            }
            Action::Copy(sha) => match copy_to_clipboard(&sha) {
                Ok(()) => {
                    if let Some(label) = git::commit_label(&self.repo, &sha) {
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
            Tab::Compare => vec![Pane::Compare],
            Tab::Review => vec![Pane::Review],
            Tab::Repo => vec![Pane::Branches, Pane::Changes, Pane::Graph],
        };
        let current = order
            .iter()
            .position(|pane| *pane == self.view.pane)
            .unwrap_or(0);
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
        self.view.in_preview = true;
    }

    /// Moves to a left-hand pane, which must be on screen, so zoom ends.
    fn focus_left(&mut self, pane: Pane) {
        self.view.zoomed = false;
        self.view.in_preview = false;
        self.view.pane = pane;
    }

    /// Shows how `a` and `b` differ on the Compare tab, replacing any
    /// comparison already there.
    fn start_compare(&mut self, a: String, b: String) {
        match ComparePanel::new(&self.repo, a, b) {
            Ok(compare) => self.show_compare(compare),
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    /// Puts a comparison, or a stash, on the Compare tab, replacing what was
    /// there, and goes to it.
    fn show_compare(&mut self, compare: ComparePanel) {
        self.go_to_tab(Tab::Compare);
        // Whatever was open on the right belonged to the old comparison.
        self.close_preview();
        self.view.zoomed = false;
        self.compare = Some(compare);
        self.focus_left(Pane::Compare);
    }

    /// Closing a comparison goes back to the Repo tab, as esc does elsewhere.
    /// From the Repo tab, as when popping the stash shown, the Repo tab stays
    /// as it is. Either way the Compare tab opens empty next time, without
    /// the old comparison's diff.
    fn close_compare(&mut self) {
        self.compare = None;
        if self.tab == Tab::Compare {
            self.go_to_tab(Tab::Repo);
        }
        self.saved_views[Tab::Compare as usize] = None;
    }

    /// Switches every diff between the whole file and just the changes. It
    /// stays that way while moving between files, for reading with context.
    fn toggle_whole_files(&mut self) {
        self.whole = !self.whole;
        // The cache keeps both kinds, so the other one, prefetched, shows
        // coloured at once.
        self.sync_whole();
    }

    /// Loads the open diff again if it's the other kind, whole or just the
    /// changes, as when `e` was pressed on another tab.
    fn sync_whole(&mut self) {
        let whole = self.whole;
        if let Some(diff) = self.view.diff_mut()
            && diff.whole != whole
        {
            let result = diff.reload_as(&self.repo, whole);
            self.report(result);
        }
    }

    /// The tabs `tab` goes through: Review only while there are comments.
    fn tabs(&self) -> Vec<Tab> {
        let mut tabs = vec![Tab::Repo, Tab::Compare];
        if !self.review.is_empty() {
            tabs.push(Tab::Review);
        }
        tabs
    }

    /// The tab `tab` leads to, or shift-tab when not `forward`.
    fn next_tab(&self, forward: bool) -> Tab {
        let tabs = self.tabs();
        let current = tabs.iter().position(|tab| *tab == self.tab).unwrap_or(0);
        let next = if forward {
            ui::next_index(current, tabs.len())
        } else {
            ui::previous_index(current, tabs.len())
        };
        tabs[next]
    }

    fn cycle_tab(&mut self, forward: bool) {
        self.go_to_tab(self.next_tab(forward));
    }

    /// Shows another tab, as it was left, putting the showing one aside.
    fn go_to_tab(&mut self, tab: Tab) {
        if tab == self.tab {
            return;
        }
        let view = self.saved_views[tab as usize]
            .take()
            .unwrap_or_else(|| View::fresh(tab));
        let showing = std::mem::replace(&mut self.view, view);
        self.saved_views[self.tab as usize] = Some(showing);
        self.tab = tab;
        self.sync_whole();
        // The Review tab opens on its selected comment's line.
        if tab == Tab::Review && self.view.diff().is_none() {
            self.show_selected_comment();
        }
    }
}

/// How often to read the repo again, after `idle` without a key.
fn refresh_every(idle: Duration) -> Duration {
    if idle < IDLE_AFTER {
        REFRESH_IN_USE
    } else {
        REFRESH_IDLE
    }
}

/// On Linux the copy may only last while gclic runs, as there the program
/// that copied holds the clipboard. macOS and Windows keep it.
fn copy_to_clipboard(text: &str) -> Result<()> {
    arboard::Clipboard::new()?.set_text(text)?;
    Ok(())
}
