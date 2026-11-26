//! The command line: the commands there are, the hints and completion as
//! one is typed, and what each runs, with the stash, fetch, repo and editor
//! actions behind them.

use super::*;

/// A command of gclic's command line, opened with `:`.
pub struct CommandInfo {
    /// As typed, with its colon: `:cd`.
    pub key: &'static str,
    /// What may follow it, if anything.
    pub args: &'static str,
    /// What it does, in a few words.
    pub about: &'static str,
}

impl CommandInfo {
    pub fn name(&self) -> &'static str {
        &self.key[1..]
    }
}

/// Every command, which the command line's hints, its completion, the help
/// and the message for an unknown command are all made from. A new command
/// goes here, besides doing something in `App::run_command`.
pub const COMMANDS: [CommandInfo; 6] = [
    CommandInfo {
        key: ":cd",
        args: "[name | ..]",
        about: "switch to another repo",
    },
    CommandInfo {
        key: ":git",
        args: "<arguments>",
        about: "run git in the background",
    },
    CommandInfo {
        key: ":!",
        args: "<command>",
        about: "run a command in your shell",
    },
    CommandInfo {
        key: ":stash",
        args: "",
        about: "list the stashes",
    },
    CommandInfo {
        key: ":fetch",
        args: "[on | off]",
        about: "fetch now, or every minute",
    },
    CommandInfo {
        key: ":color",
        args: "[#rrggbb | reset | on | off]",
        about: "this repo's colour",
    },
];

/// The commands as the help lists them: each with what it does.
pub const COMMAND_HELP: [(&str, &str); COMMANDS.len()] = {
    let mut help = [("", ""); COMMANDS.len()];
    let mut i = 0;
    while i < COMMANDS.len() {
        help[i] = (COMMANDS[i].key, COMMANDS[i].about);
        i += 1;
    }
    help
};

/// `:cd, :git, …`, for the message about an unknown command.
pub fn command_list() -> String {
    COMMANDS
        .iter()
        .map(|command| command.key)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The commands whose names start with what's typed.
fn matching(typed: &str) -> Vec<&'static CommandInfo> {
    COMMANDS
        .iter()
        .filter(|command| command.name().starts_with(typed))
        .collect()
}

/// Names, separated by dots, as many as fit in `width`, then `…`.
fn fit_names(names: &[&str], width: usize) -> String {
    let mut text = String::new();
    for (i, name) in names.iter().enumerate() {
        let next = if text.is_empty() {
            name.to_string()
        } else {
            format!("{text} · {name}")
        };
        // Room for " · …" after it, unless it's the last.
        let room = if i + 1 == names.len() {
            width
        } else {
            width.saturating_sub(4)
        };
        if next.chars().count() > room {
            return if text.is_empty() {
                "…".to_string()
            } else {
                format!("{text} · …")
            };
        }
        text = next;
    }
    text
}

/// What the command line shows after the cursor, in dim text, for what's
/// typed so far: the rest of the only command it can be, which tab fills in,
/// and a hint in `width` columns. With nothing typed, the commands' names;
/// while typing a name, the ones it matches, or once only one does, its
/// arguments and what it does, which stay until its arguments are typed.
pub fn command_hint(line: &str, width: usize) -> (String, String) {
    let names: Vec<&str> = COMMANDS.iter().map(CommandInfo::name).collect();
    let about = |command: &CommandInfo| {
        if command.args.is_empty() {
            command.about.to_string()
        } else {
            format!("{}   {}", command.args, command.about)
        }
    };
    if line.is_empty() {
        let keys = "   tab completes · esc cancel";
        let list = fit_names(&names, width);
        return if list.chars().count() + keys.chars().count() <= width && !list.ends_with('…') {
            (String::new(), format!("{list}{keys}"))
        } else {
            (String::new(), list)
        };
    }
    // `!` takes the whole line, with no space after it.
    if let Some(rest) = line.strip_prefix('!') {
        let shell = COMMANDS.iter().find(|command| command.name() == "!");
        let hint = match shell {
            Some(command) if rest.is_empty() => about(command),
            _ => String::new(),
        };
        return (String::new(), hint);
    }
    let (word, rest) = match line.split_once(' ') {
        Some((word, rest)) => (word, Some(rest)),
        None => (line, None),
    };
    let fits = |text: String| {
        if text.chars().count() <= width {
            text
        } else {
            String::new()
        }
    };
    match rest {
        // The arguments, until they're typed.
        Some(rest) => match COMMANDS.iter().find(|command| command.name() == word) {
            Some(command) if rest.trim().is_empty() => (String::new(), fits(about(command))),
            _ => (String::new(), String::new()),
        },
        None => match matching(word).as_slice() {
            [] => (String::new(), "no such command".to_string()),
            [command] => {
                let ghost = command.name()[word.len()..].to_string();
                let keys = if ghost.is_empty() {
                    ""
                } else {
                    "   tab completes"
                };
                let hint = fits(format!("  {}{keys}", about(command)));
                (ghost, hint.trim_start().to_string())
            }
            several => {
                let names: Vec<&str> = several.iter().map(|command| command.name()).collect();
                (String::new(), fit_names(&names, width))
            }
        },
    }
}

/// What tab makes of a partly typed command name: the name, with a space if
/// it takes arguments, once only one command matches, or the part all the
/// matching ones share. None when there's nothing to add.
pub fn complete_command(line: &str) -> Option<String> {
    if line.contains(' ') || line.starts_with('!') {
        return None;
    }
    let found = matching(line);
    let completed = match found.as_slice() {
        [] => return None,
        [command] if command.args.is_empty() => command.name().to_string(),
        [command] => format!("{} ", command.name()),
        several => {
            let first = several[0].name();
            let shared = (line.len()..=first.len())
                .rev()
                .find(|&end| {
                    several
                        .iter()
                        .all(|command| command.name().starts_with(&first[..end]))
                })
                .unwrap_or(line.len());
            first[..shared].to_string()
        }
    };
    (completed != line).then_some(completed)
}

impl App {
    /// Typing on the command line. Enter runs it, and esc, or backspace with
    /// nothing left, closes it.
    pub(super) fn handle_command_key(&mut self, mut line: String, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.run_command(&line),
            KeyCode::Esc => {}
            // A suggestion from git first, or else the command being typed.
            KeyCode::Tab => {
                if let Some(suggestion) = self
                    .suggestion
                    .as_ref()
                    .filter(|suggestion| suggestion.starts_with(&line))
                {
                    line = suggestion.clone();
                } else if let Some(completed) = complete_command(&line) {
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
            _ => self.message = format!("Unknown command :{name}. Try {}", command_list()),
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
                let from = if self.accent.custom {
                    "set with :color"
                } else {
                    "from the repo's name"
                };
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
                        self.accent_bar = accent::bar_shown(&self.repo);
                        self.tell_editor_about_color();
                        self.message = match (off, self.accent_bar) {
                            (true, false) => {
                                "The colour bar is off in every repo. :color on shows it again"
                            }
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
                    self.message = format!(
                        "{hex} isn't a colour. Try :color #rrggbb, as in :color #e0a040, :color reset or :color off"
                    );
                    return;
                }
            },
        };
        let value = value.map(|hex| format!("#{hex}"));
        match git::set_local_config(&self.repo, accent::CONFIG_KEY, value.as_deref()) {
            Ok(()) => {
                self.accent = Accent::of(&self.repo);
                self.tell_editor_about_color();
                let colour = match value {
                    Some(_) => format!("This repo's colour is now {}", self.accent.hex()),
                    None => format!(
                        "This repo's colour is back to {}, from its name",
                        self.accent.hex()
                    ),
                };
                let hidden = if self.accent_bar {
                    ""
                } else {
                    ". The bar is off: :color on shows it"
                };
                self.message = format!("{colour}{hidden}");
            }
            Err(error) => self.message = format!("Couldn't save the colour: {error}"),
        }
    }

    /// `:git …`: runs in the background, without a terminal, and shows what
    /// it printed when it ends. One at a time, as git commands often lock.
    pub(super) fn run_git(&mut self, args: &str) {
        if let Some(running) = &self.running {
            self.message = format!(
                "{} is still running: wait, or stop it with ctrl-c",
                running.label
            );
            return;
        }
        let args = match crate::args::split_args(args) {
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
        match Running::start(label, git::user_command(&self.repo, &args)) {
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
        let stashes = match git::stashes(&self.repo) {
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
            .map(|stash| {
                (
                    format!("{}  {}  {}", stash.name, stash.message, stash.age),
                    stash,
                )
            })
            .collect();
        let labels = stashes.iter().map(|(label, _)| label.clone()).collect();
        let picker = Picker::single("Stashes", labels).with_hints(&STASH_PICKER_KEYS);
        self.modal = Modal::Picker(picker, PickerUse::Stash(stashes));
    }

    /// Shows a stash's files, and their diffs, where comparisons show theirs.
    pub(super) fn open_stash(&mut self, stash: Stash) {
        match ComparePanel::stash(&self.repo, stash) {
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
            StashAction::Apply => git::stash_apply(&self.repo, &stash),
            StashAction::Pop => git::stash_pop(&self.repo, &stash),
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
        let shown = self
            .compare
            .as_ref()
            .and_then(|compare| compare.stash_shown());
        if action != StashAction::Apply && shown.is_some_and(|shown| shown.sha == stash.sha) {
            self.close_compare();
        }
    }

    /// `:fetch` fetches now, and `:fetch on` or `off` turns fetching every
    /// minute on or off, for the session.
    pub(super) fn fetch_command(&mut self, setting: &str) {
        match setting {
            "" if self.fetcher.is_fetching() => self.message = "Already fetching".to_string(),
            "" => self.fetcher.start(),
            "on" => {
                self.fetcher.auto = true;
                self.message = "Fetching every minute".to_string();
            }
            "off" => {
                self.fetcher.auto = false;
                self.message = "Not fetching any more, until :fetch on".to_string();
            }
            _ => self.message = "Try :fetch, :fetch on or :fetch off".to_string(),
        }
    }

    /// Picks up a `:git` command that has ended. gclic refreshes, as it may
    /// have changed anything. A line or less of output, after it worked,
    /// goes to the message line, and anything else to a popup, unless another
    /// popup is open, which it isn't worth taking over. Whether one ended.
    pub(super) fn take_command_result(&mut self) -> bool {
        let Some(finished) = self.running.as_ref().and_then(Running::finished) else {
            return false;
        };
        let Some(Running { label, stopped, .. }) = self.running.take() else {
            return false;
        };
        self.refresh();
        if stopped {
            self.message = format!("{label}: stopped");
        } else if finished.succeeded() && finished.lines.len() <= 1 {
            let line = finished
                .lines
                .first()
                .map(|line| crate::ansi::strip_ansi(line));
            self.message = line.unwrap_or_else(|| format!("{label}: done"));
        } else if matches!(self.modal, Modal::None) {
            let output = OutputPopup::new(&label, finished);
            self.suggestion = output.suggestion();
            self.modal = Modal::Output(output);
        } else {
            let outcome = if finished.succeeded() {
                "finished"
            } else {
                "failed"
            };
            self.message = format!("{label} {outcome}");
        }
        true
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
                self.message = format!(
                    "No repo matches {name} in {}",
                    repos::tilde(&self.search_root)
                );
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
        let mut picker = Picker::single(&title, labels)
            .with_hints(&repos::PICKER_KEYS)
            .with_dimmed_folders();
        if !query.is_empty() {
            picker.set_query(query.to_string());
        }
        let current = self.repo.root();
        if query.is_empty()
            && let Some((label, _)) = repos
                .iter()
                .find(|(_, repo)| repos::same_folder(repo, current))
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
    pub(super) fn switch_repo(&mut self, folder: &Path, tell_nvim: bool) {
        let Some(repo) = Repo::discover(folder) else {
            self.modal = Modal::Error(format!(
                "Can't open {}: it isn't a git repo",
                folder.display()
            ));
            return;
        };
        let mut fresh = App::new(repo, self.search_root.clone());
        // Inside Neovim, its project follows, for its file finder, search and
        // file tree.
        if let Some(address) = editor::parent_nvim().filter(|_| tell_nvim) {
            fresh.start_in_background("nvim", editor::cd_nvim(&address, folder));
        }
        fresh.split = self.split;
        fresh.fetcher.auto = self.fetcher.auto;
        // A command still running carries on, and its result shows here.
        fresh.running = self.running.take();
        fresh.follow = self.follow.take();
        // The old worker stops once it finds no one listening for its results.
        *self = fresh;
    }

    /// Opens the focused file in the user's editor, or in the Neovim gclic runs
    /// in, while gclic carries on. It opens at the diff's active line, or at
    /// the file's first change without a diff. History opens the file as it is now, so the line
    /// may have moved since.
    pub(super) fn open_in_editor(&mut self) {
        let Some(source) = self.focused_file() else {
            return;
        };
        let line = match self.view.diff() {
            Some(diff) if diff.source == source => diff.active_line(),
            _ => DiffPane::load(&self.repo, self.view_of(&source))
                .ok()
                .and_then(|diff| diff.active_line()),
        };
        let path = self.repo.file(source.path());
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
            // Starting in the repo, for an editor's own file finder and the
            // like, wherever gclic itself was started.
            Ok(Launch::Background(mut command)) => {
                command.current_dir(self.repo.root());
                self.start_in_background(&editor::name(&setting), command);
            }
            Ok(Launch::Terminal(shown, mut command)) => {
                command.current_dir(self.repo.root());
                self.editor_in_terminal = Some((shown, editor::name(&setting), command));
            }
            Err(error) => self.message = format!("The editor setting doesn't work: {error}"),
        }
    }

    /// Starts an editor, or a message to one, leaving it running, to collect
    /// once it ends.
    pub(super) fn start_in_background(&mut self, name: &str, mut command: Command) {
        let started = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match started {
            Ok(child) => self.editor_commands.push(child),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.message = format!("{name} isn't installed, or isn't on the PATH");
            }
            Err(error) => self.modal = Modal::Error(format!("Couldn't run {name}: {error}")),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_nothing_typed_the_hint_lists_the_commands_as_far_as_they_fit() {
        let (_, wide) = command_hint("", 100);
        assert_eq!(
            wide,
            "cd · git · ! · stash · fetch · color   tab completes · esc cancel"
        );
        let (_, narrow) = command_hint("", 20);
        assert_eq!(narrow, "cd · git · ! · …");
        assert!(narrow.chars().count() <= 20);
    }

    #[test]
    fn typing_a_name_narrows_the_hint_down_to_one_command() {
        assert_eq!(
            command_hint("c", 100),
            (String::new(), "cd · color".to_string())
        );
        let (ghost, hint) = command_hint("co", 100);
        assert_eq!(ghost, "lor");
        assert_eq!(
            hint,
            "[#rrggbb | reset | on | off]   this repo's colour   tab completes"
        );
        assert_eq!(command_hint("zz", 100).1, "no such command");
    }

    #[test]
    fn arguments_are_shown_until_they_are_typed() {
        assert_eq!(
            command_hint("fetch ", 100).1,
            "[on | off]   fetch now, or every minute"
        );
        assert_eq!(command_hint("fetch o", 100).1, "");
        assert_eq!(command_hint("stash", 100).1, "list the stashes");
        assert_eq!(
            command_hint("!", 100).1,
            "<command>   run a command in your shell"
        );
        assert_eq!(command_hint("!ls", 100).1, "");
    }

    #[test]
    fn tab_completes_a_command_name() {
        assert_eq!(complete_command("co").as_deref(), Some("color "));
        assert_eq!(complete_command("st").as_deref(), Some("stash"));
        // Only as far as the names share.
        assert_eq!(complete_command("c"), None);
        assert_eq!(complete_command("color "), None);
    }

    #[test]
    fn every_command_is_in_the_help_and_the_unknown_command_message() {
        assert_eq!(COMMAND_HELP.len(), COMMANDS.len());
        assert_eq!(command_list(), ":cd, :git, :!, :stash, :fetch, :color");
    }
}
