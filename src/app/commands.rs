//! What `:` runs, and the stash, fetch, repo and editor actions behind it.

use super::*;

impl App {
    /// Typing on the command line. Enter runs it, and esc, or backspace with
    /// nothing left, closes it.
    pub(super) fn handle_command_key(&mut self, mut line: String, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.run_command(&line),
            KeyCode::Esc => {}
            // A suggestion from git first, or else the command being typed.
            KeyCode::Tab => {
                if let Some(suggestion) = self.suggestion.as_ref().filter(|suggestion| suggestion.starts_with(&line)) {
                    line = suggestion.clone();
                } else if let Some(completed) = crate::commands::complete_command(&line) {
                    line = completed;
                }
                self.modal = Modal::Command(line);
            }
            KeyCode::Backspace if line.is_empty() => {}
            KeyCode::Backspace => {
                line.pop();
                self.modal = Modal::Command(line);
            }
            KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                line.push(ch);
                self.modal = Modal::Command(line);
            }
            _ => self.modal = Modal::Command(line),
        }
    }

    /// Runs a command typed after `:`. The first word names it, and the rest
    /// are its arguments, except after `!`, which takes the whole line.
    pub(super) fn run_command(&mut self, line: &str) {
        let line = line.trim();
        self.suggestion = None;
        if let Some(shell_line) = line.strip_prefix('!') {
            if !shell_line.trim().is_empty() {
                self.shell_line = Some(shell_line.trim().to_string());
            }
            return;
        }
        let (name, rest) = line.split_once(' ').unwrap_or((line, ""));
        match name {
            "" => {}
            "cd" => self.change_repo(rest.trim()),
            "git" => self.run_git(rest),
            "fetch" => self.fetch_command(rest.trim()),
            "stash" => self.open_stash_picker(),
            "color" | "colour" => self.color_command(rest.trim()),
            _ => self.message = format!("Unknown command :{name}. Try {}", crate::commands::command_list()),
        }
    }

    /// `:color`: says the repo's colour. `:color #rrggbb` sets it, in the
    /// repo's local git config, and `:color reset` goes back to the one from
    /// the repo's name. `:color off` hides the bar in every repo, in the
    /// user's git config, and `:color on` shows it again.
    pub(super) fn color_command(&mut self, arg: &str) {
        let value = match arg {
            "" if !self.accent_bar => {
                self.message = "The colour bar is off. :color on shows it".to_string();
                return;
            }
            "" => {
                let from = if self.accent.custom { "set with :color" } else { "from the repo's name" };
                self.message = format!(
                    "This repo's colour is {}, {from}. :color #rrggbb sets another, :color off hides the bar",
                    self.accent.hex()
                );
                return;
            }
            "off" | "on" => {
                let off = arg == "off";
                match git::set_global_config(accent::BAR_KEY, off.then_some("false")) {
                    Ok(()) => {
                        self.accent_bar = accent::bar_shown();
                        self.tell_editor_about_color();
                        self.message = match (off, self.accent_bar) {
                            (true, false) => "The colour bar is off in every repo. :color on shows it again",
                            (false, true) => "The colour bar is on",
                            // A repo's own config can still say otherwise.
                            _ => "This repo's git config overrides that, with gclic.colorBar",
                        }
                        .to_string();
                    }
                    Err(error) => self.message = format!("Couldn't save that: {error}"),
                }
                return;
            }
            "reset" => None,
            hex => match accent::parse_hex(hex) {
                Some(_) => Some(hex.trim_start_matches('#').to_lowercase()),
                None => {
                    self.message =
                        format!("{hex} isn't a colour. Try :color #rrggbb, as in :color #e0a040, :color reset or :color off");
                    return;
                }
            },
        };
        let value = value.map(|hex| format!("#{hex}"));
        match git::set_local_config(accent::CONFIG_KEY, value.as_deref()) {
            Ok(()) => {
                self.accent = Accent::current();
                self.tell_editor_about_color();
                let colour = match value {
                    Some(_) => format!("This repo's colour is now {}", self.accent.hex()),
                    None => format!("This repo's colour is back to {}, from its name", self.accent.hex()),
                };
                let hidden = if self.accent_bar { "" } else { ". The bar is off: :color on shows it" };
                self.message = format!("{colour}{hidden}");
            }
            Err(error) => self.message = format!("Couldn't save the colour: {error}"),
        }
    }

    /// `:git …`: runs in the background, without a terminal, and shows what
    /// it printed when it ends. One at a time, as git commands often lock.
    pub(super) fn run_git(&mut self, args: &str) {
        if let Some(running) = &self.running {
            self.message = format!("{} is still running: wait, or stop it with ctrl-c", running.label);
            return;
        }
        let args = match crate::commands::split_args(args) {
            Ok(args) if args.is_empty() => {
                self.message = "Type a git command after :git, as in :git fetch".to_string();
                return;
            }
            Ok(args) => args,
            Err(error) => {
                self.message = error;
                return;
            }
        };
        let label = format!("git {}", args.join(" "));
        match Running::start(label, git::user_command(&args)) {
            Ok(running) => self.running = Some(running),
            Err(error) => self.modal = Modal::Error(format!("Couldn't run git: {error}")),
        }
    }

    /// Inside Neovim, has it show the repo's colour as it now is.
    pub(super) fn tell_editor_about_color(&mut self) {
        if let Some(address) = editor::parent_nvim() {
            self.start_in_background("nvim", editor::refresh_nvim_accent(&address));
        }
    }

    /// `:stash`: the stashes, to open one's files, or apply, pop or drop it.
    pub(super) fn open_stash_picker(&mut self) {
        let stashes = match git::stashes() {
            Ok(stashes) => stashes,
            Err(error) => {
                self.modal = Modal::Error(error.to_string());
                return;
            }
        };
        if stashes.is_empty() {
            self.message = "No stashes. :git stash puts your changes aside".to_string();
            return;
        }
        let stashes: Vec<(String, Stash)> = stashes
            .into_iter()
            .map(|stash| (format!("{}  {}  {}", stash.name, stash.message, stash.age), stash))
            .collect();
        let labels = stashes.iter().map(|(label, _)| label.clone()).collect();
        let picker = Picker::single("Stashes", labels).with_hints(&STASH_PICKER_KEYS);
        self.modal = Modal::Picker(picker, PickerUse::Stash(stashes));
    }

    /// Shows a stash's files, and their diffs, where comparisons show theirs.
    pub(super) fn open_stash(&mut self, stash: Stash) {
        match ComparePanel::stash(stash) {
            Ok(view) => self.show_compare(view),
            Err(error) => self.modal = Modal::Error(error.to_string()),
        }
    }

    /// Applies, pops or drops a stash, dropping only after asking. The stash
    /// view closes on a stash that's gone, back to the others.
    pub(super) fn act_on_stash(&mut self, action: StashAction, stash: Stash) {
        let result = match action {
            StashAction::Drop => {
                self.modal = Modal::Confirm(Confirm::DropStash(stash));
                return;
            }
            StashAction::Apply => git::stash_apply(&stash),
            StashAction::Pop => git::stash_pop(&stash),
        };
        self.after_stash(action, &stash, result);
    }

    pub(super) fn after_stash(&mut self, action: StashAction, stash: &Stash, result: Result<()>) {
        let done = result.is_ok();
        self.apply(Action::after_change(result));
        if !done {
            return;
        }
        self.message = match action {
            StashAction::Apply => format!("Applied {}", stash.name),
            StashAction::Pop => format!("Popped {}", stash.name),
            StashAction::Drop => format!("Dropped {}", stash.name),
        };
        let shown = self.compare.as_ref().and_then(|compare| compare.stash_shown());
        if action != StashAction::Apply && shown.is_some_and(|shown| shown.sha == stash.sha) {
            self.close_compare();
        }
    }

    /// `:fetch` fetches now, and `:fetch on` or `off` turns fetching every
    /// minute on or off, for the session.
    pub(super) fn fetch_command(&mut self, setting: &str) {
        match setting {
            "" if self.fetching.is_some() => self.message = "Already fetching".to_string(),
            "" => self.start_fetch(),
            "on" => {
                self.auto_fetch = true;
                self.message = "Fetching every minute".to_string();
            }
            "off" => {
                self.auto_fetch = false;
                self.message = "Not fetching any more, until :fetch on".to_string();
            }
            _ => self.message = "Try :fetch, :fetch on or :fetch off".to_string(),
        }
    }

    /// Fetches the remotes the branches track, on a thread of its own, as it
    /// can take seconds, which would hold up the worker's refreshes.
    pub(super) fn start_fetch(&mut self) {
        let (send, receive) = mpsc::channel();
        thread::spawn(move || {
            let _ = send.send(git::background_fetch(&git::tracked_remotes()));
        });
        self.fetching = Some(receive);
        self.fetch_started = Some(Instant::now());
    }

    /// Picks up a fetch that has ended, and starts the next when it's due. A
    /// fetch that brought anything moves the remote refs, which the next poll
    /// sees, refreshing everything.
    pub(super) fn keep_fetching(&mut self) {
        if let Some(fetching) = &self.fetching {
            match fetching.try_recv() {
                Ok(result) => {
                    self.fetched = Some(result.map(|()| Instant::now()).map_err(|error| error.to_string()));
                    self.fetching = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.fetched = Some(Err("the fetch crashed".to_string()));
                    self.fetching = None;
                }
            }
        }
        let due = self.fetch_started.is_none_or(|started| started.elapsed() >= FETCH_EVERY);
        if self.auto_fetch && self.fetching.is_none() && due {
            self.start_fetch();
        }
    }

    /// What the Branches title says about fetching: how long ago it last
    /// worked, or that it failed. Nothing before the first fetch ends.
    pub(super) fn fetch_note(&self) -> Option<String> {
        match &self.fetched {
            None if self.fetching.is_some() => Some("fetching…".to_string()),
            None => None,
            Some(Ok(finished)) => Some(fetched_ago(finished.elapsed())),
            Some(Err(_)) => Some("fetch failed".to_string()),
        }
    }

    /// Picks up a `:git` command that has ended. gclic refreshes, as it may
    /// have changed anything. A line or less of output, after it worked,
    /// goes to the message line, and anything else to a popup, unless another
    /// popup is open, which it isn't worth taking over.
    pub(super) fn take_command_result(&mut self) {
        let Some(finished) = self.running.as_ref().and_then(Running::finished) else {
            return;
        };
        let Some(Running { label, stopped, .. }) = self.running.take() else {
            return;
        };
        self.refresh();
        if stopped {
            self.message = format!("{label}: stopped");
        } else if finished.succeeded() && finished.lines.len() <= 1 {
            let line = finished.lines.first().map(|line| crate::commands::strip_ansi(line));
            self.message = line.unwrap_or_else(|| format!("{label}: done"));
        } else if matches!(self.modal, Modal::None) {
            let output = OutputPopup::new(&label, finished);
            self.suggestion = output.suggestion();
            self.modal = Modal::Output(output);
        } else {
            let outcome = if finished.succeeded() { "finished" } else { "failed" };
            self.message = format!("{label} {outcome}");
        }
    }

    /// `:cd`: on its own, the finder. With `..`, the finder a folder up. With
    /// a name, straight to the repo it can only mean, or else the finder with
    /// the name typed in.
    pub(super) fn change_repo(&mut self, name: &str) {
        if name.is_empty() {
            return self.open_repo_picker("");
        }
        if name == repos::UP {
            if let Some(parent) = self.search_root.parent() {
                self.search_root = parent.to_path_buf();
            }
            return self.open_repo_picker("");
        }
        let entries = repos::entries(&self.search_root);
        match repos::target(name, &entries) {
            repos::Target::Repo(repo) => self.switch_repo(&repo, true),
            repos::Target::Choose => self.open_repo_picker(name),
            repos::Target::None => {
                self.message = format!("No repo matches {name} in {}", repos::tilde(&self.search_root));
            }
        }
    }

    /// The repos under the search folder, to switch to one, after `..` to
    /// search a folder up. Opens as the other pickers do, with `query` as the
    /// search if there is one, and otherwise on the current repo.
    pub(super) fn open_repo_picker(&mut self, query: &str) {
        let repos = repos::entries(&self.search_root);
        let labels = repos.iter().map(|(label, _)| label.clone()).collect();
        let title = format!("Switch repo, in {}", repos::tilde(&self.search_root));
        let mut picker = Picker::single(&title, labels).with_hints(&repos::PICKER_KEYS).with_dimmed_folders();
        if !query.is_empty() {
            picker.set_query(query.to_string());
        }
        let current = std::env::current_dir().ok();
        if query.is_empty()
            && let Some((label, _)) = repos.iter().find(|(_, repo)| Some(repo) == current.as_ref())
        {
            picker.select_item(&label.clone());
        }
        self.modal = Modal::Picker(picker, PickerUse::Repo(repos));
    }

    /// Makes another repo the one gclic shows and runs git in. Everything
    /// read from the old one goes, so the app starts afresh, keeping only the
    /// user's choices of how to show things.
    /// `tell_nvim` is false when following Neovim, which is already there,
    /// perhaps in a folder inside the repo, which telling it would undo.
    pub(super) fn switch_repo(&mut self, repo: &Path, tell_nvim: bool) {
        if let Err(error) = std::env::set_current_dir(repo) {
            self.modal = Modal::Error(format!("Can't open {}: {error}", repo.display()));
            return;
        }
        let mut fresh = App::new(self.search_root.clone());
        // Inside Neovim, its project follows, for its file finder, search and
        // file tree.
        if let Some(address) = editor::parent_nvim().filter(|_| tell_nvim) {
            fresh.start_in_background("nvim", editor::cd_nvim(&address, repo));
        }
        fresh.split = self.split;
        fresh.auto_fetch = self.auto_fetch;
        // A command still running carries on, and its result shows here.
        fresh.running = self.running.take();
        fresh.follow = self.follow.take();
        // The old worker stops once it finds no one listening for its results.
        *self = fresh;
    }

    /// Opens the focused file in Zed, which comes to the front, while gclic
    /// carries on. It opens at the diff's active line, or at the file's first
    /// change without a diff. History opens the file as it is now, so the line
    /// may have moved since.
    pub(super) fn open_in_editor(&mut self) {
        let Some(source) = self.focused_file() else {
            return;
        };
        let line = match self.view.diff() {
            Some(diff) if diff.source == source => diff.active_line(),
            _ => DiffPane::load(self.view_of(&source)).ok().and_then(|diff| diff.active_line()),
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
        let line = line.unwrap_or(1);
        // Inside Neovim, the file opens there rather than in a second editor.
        if let Some(address) = editor::parent_nvim() {
            return self.start_in_background("nvim", editor::open_in_nvim(&address, &path, line));
        }
        let setting = editor::setting();
        match editor::launch(&setting, &path, line) {
            Ok(Launch::Background(command)) => self.start_in_background(&editor::name(&setting), command),
            Ok(Launch::Terminal(shown, command)) => {
                self.editor_in_terminal = Some((shown, editor::name(&setting), command));
            }
            Err(error) => self.message = format!("The editor setting doesn't work: {error}"),
        }
    }

    /// Starts an editor, or a message to one, leaving it running, to collect
    /// once it ends.
    pub(super) fn start_in_background(&mut self, name: &str, mut command: Command) {
        let started = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        match started {
            Ok(child) => self.editor_commands.push(child),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.message = format!("{name} isn't installed, or isn't on the PATH");
            }
            Err(error) => self.modal = Modal::Error(format!("Couldn't run {name}: {error}")),
        }
    }
}

/// How long ago, in minutes or hours, or under a minute in steps of ten
/// seconds, so it doesn't tick away at the edge of the eye.
pub(super) fn fetched_ago(elapsed: Duration) -> String {
    match elapsed.as_secs() {
        0..10 => "fetched just now".to_string(),
        seconds @ 10..60 => format!("fetched {}s ago", seconds / 10 * 10),
        seconds @ 60..3600 => format!("fetched {}m ago", seconds / 60),
        seconds => format!("fetched {}h ago", seconds / 3600),
    }
}

/// The first word on the clipboard, which is all a ref could be. None if it
/// holds no text.
pub(super) fn read_clipboard() -> Result<Option<String>> {
    match arboard::Clipboard::new()?.get_text() {
        Ok(text) => Ok(text.split_whitespace().next().map(str::to_string)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
