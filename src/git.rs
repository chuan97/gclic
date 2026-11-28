//! Every interaction with git lives here. The rest of the app never sees a
//! subprocess or raw git output, only the typed data returned by this module.

use anyhow::{Result, anyhow, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

/// A repo gclic works in: every git call runs in its root, whatever the
/// process's working directory, so two repos, or a repo and a test, never get
/// in each other's way.
#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    root: PathBuf,
    /// Some questions are answered by reading its files, which is much quicker
    /// than starting git.
    git_dir: PathBuf,
}

impl Repo {
    /// The repo `folder` is in, if it's in one with a working tree.
    pub fn discover(folder: &Path) -> Option<Repo> {
        let text = output(
            command(&["rev-parse", "--show-toplevel", "--absolute-git-dir"]).current_dir(folder),
        )
        .ok()?;
        let mut lines = text.lines();
        let root = PathBuf::from(lines.next()?.trim());
        let git_dir = PathBuf::from(lines.next()?.trim());
        Some(Repo { root, git_dir })
    }

    /// The folder at the top of its working tree, which paths in it are
    /// relative to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A file of the working tree, by its path from the root.
    pub fn file(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    /// For tests of what runs no git: a repo that isn't there, so a git call
    /// that slips in fails rather than reads some other repo.
    #[cfg(test)]
    pub fn nowhere() -> Repo {
        let root = std::env::temp_dir().join("gclic-no-repo-here");
        Repo {
            git_dir: root.join(".git"),
            root,
        }
    }

    /// A git command run in the repo's root.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = command(args);
        command.current_dir(&self.root);
        command
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FileEntry {
    /// Status letter: M, A, D, ... plus `?` for untracked and `U` for conflicts.
    pub status: char,
    pub path: String,
    pub lines: Lines,
}

/// How many lines a file's change adds and removes, where it's known: for a
/// commit's, a stash's or a comparison's files, not the working tree's.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Lines {
    #[default]
    Unknown,
    Binary,
    Counted {
        added: usize,
        removed: usize,
    },
}

#[derive(Debug, Default, PartialEq)]
pub struct Status {
    /// Files with merge conflicts, which are neither staged nor plain changes.
    pub conflicted: Vec<FileEntry>,
    pub staged: Vec<FileEntry>,
    pub unstaged: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct CommitDetails {
    pub sha: String,
    pub author: String,
    pub date: String,
    pub message: String,
    /// How many files it changes, and the lines it adds and removes in them,
    /// against its first parent for a merge, as the graph lists its files.
    pub files: usize,
    pub lines: Lines,
}

#[derive(Debug, Clone)]
pub struct Branch {
    /// Short name, e.g. `main` or `origin/main`.
    pub name: String,
    /// Name without the remote, e.g. `main` for `origin/main`.
    pub local_name: String,
    pub is_remote: bool,
    /// The remote branch this one tracks, for a local branch that has one.
    pub upstream: Option<String>,
    /// How many commits it's ahead of and behind its upstream. None without
    /// one, or if the upstream was deleted on the remote.
    pub ahead_behind: Option<(usize, usize)>,
    /// The worktree it's checked out in, for a local branch that is, which
    /// may be this one or another.
    pub worktree: Option<String>,
}

/// What a diff pane is showing. One variant per place a diff can come from.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffSource {
    Unstaged(String),
    Staged(String),
    Untracked(String),
    /// A file with merge conflicts: its two sides, from the conflict markers.
    Conflict(String),
    Commit {
        sha: String,
        path: String,
    },
    Refs {
        a: String,
        b: String,
        path: String,
    },
    /// A file in a stash: tracked changes against the commit it was made on,
    /// or an untracked file, kept in a commit of its own.
    Stash {
        sha: String,
        path: String,
        untracked: bool,
    },
}

impl DiffSource {
    /// Whether this comes from the Changes pane, rather than from history.
    pub fn is_working_tree(&self) -> bool {
        matches!(
            self,
            DiffSource::Unstaged(_)
                | DiffSource::Staged(_)
                | DiffSource::Untracked(_)
                | DiffSource::Conflict(_)
        )
    }

    /// The file's path, relative to the repo root.
    pub fn path(&self) -> &str {
        match self {
            DiffSource::Unstaged(path)
            | DiffSource::Staged(path)
            | DiffSource::Untracked(path)
            | DiffSource::Conflict(path) => path,
            DiffSource::Commit { path, .. }
            | DiffSource::Refs { path, .. }
            | DiffSource::Stash { path, .. } => path,
        }
    }
}

/// A git command as gclic runs its own, in no repo in particular: `Repo`
/// gives it one.
fn command(args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(args)
        // Read-only refreshes shouldn't take the index lock and get in the
        // way of git commands the user runs in another terminal.
        .env("GIT_OPTIONAL_LOCKS", "0")
        // Paths are file names, not patterns: discarding a file named `[ab]`
        // or `*` mustn't take `a`, `b` or every other file with it.
        .env("GIT_LITERAL_PATHSPECS", "1")
        // Nothing may wait for input on the terminal the TUI is drawing on.
        .stdin(Stdio::null());
    #[cfg(test)]
    without_user_config(&mut command);
    command
}

/// Leaves out the user's and the system's git settings, for tests, which run
/// on repos made for them with settings of their own: a setting of the
/// user's, a hook or commit signing, say, could make them fail.
#[cfg(test)]
pub fn without_user_config(command: &mut Command) {
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
}

/// Runs a command to the end: what it printed, or why it failed.
fn output(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .map_err(|e| anyhow!("could not run git: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Most failures explain themselves on stderr, but `git commit` reports
        // things like "nothing to commit" on stdout.
        let text = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        bail!("{}", text.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run(repo: &Repo, args: &[&str]) -> Result<String> {
    output(&mut repo.command(args))
}

/// Runs a command for what it does, not what it prints.
fn run_ok(repo: &Repo, args: &[&str]) -> Result<()> {
    run(repo, args).map(|_| ())
}

/// Like `run`, but returns None instead of an error when the command fails.
/// For questions where failing is a normal answer, like "is there an upstream?".
fn try_run(repo: &Repo, args: &[&str]) -> Option<String> {
    run(repo, args).ok().map(|text| text.trim().to_string())
}

/// A setting from the repo's own git config, `.git/config`, ignoring the
/// user's and the system's.
pub fn local_config(repo: &Repo, key: &str) -> Option<String> {
    try_run(repo, &["config", "--local", "--get", key]).filter(|value| !value.is_empty())
}

/// A true or false setting, from wherever git finds it, the repo's own
/// config first. None if it isn't set, or isn't a boolean.
pub fn config_bool(repo: &Repo, key: &str) -> Option<bool> {
    match try_run(repo, &["config", "--get", "--type=bool", key])?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Sets a setting in `.git/config`, or with None, removes it.
pub fn set_local_config(repo: &Repo, key: &str, value: Option<&str>) -> Result<()> {
    set_config(|args| repo.command(args), "--local", key, value)
}

/// Sets a setting in the user's own git config, for every repo, or with
/// None, removes it.
pub fn set_global_config(key: &str, value: Option<&str>) -> Result<()> {
    set_config(command, "--global", key, value)
}

fn set_config(
    command: impl Fn(&[&str]) -> Command,
    scope: &str,
    key: &str,
    value: Option<&str>,
) -> Result<()> {
    match value {
        Some(value) => output(&mut command(&["config", scope, key, value])).map(|_| ()),
        // Removing one that isn't there fails, which is fine.
        None => {
            let _ = command(&["config", scope, "--unset", key]).output();
            Ok(())
        }
    }
}

/// A git command the user typed on gclic's command line, set up to run
/// without a terminal: the TUI is drawing on it. Anything that would ask for
/// something fails at once instead of waiting forever: the editor, passwords,
/// and SSH passphrases. Keys in an SSH agent still work. Unlike gclic's own
/// reads, it takes locks as git normally does.
pub fn user_command(repo: &Repo, args: &[String]) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(&repo.root)
        // Git leaves colour out when its output isn't a terminal, and gclic
        // shows it, as the user would see it in one.
        .args(["-c", "color.ui=always"])
        .args(args)
        // A command with a space runs through git's shell, on every platform.
        // Git adds the file to edit after it, which the # makes a comment.
        .env("GIT_EDITOR", "exit 1 #")
        .env("GIT_SEQUENCE_EDITOR", "exit 1 #")
        .env("GIT_PAGER", "cat");
    no_prompts(repo, &mut command);
    #[cfg(test)]
    without_user_config(&mut command);
    command
}

/// Makes a command that reaches a remote fail rather than ask for a password
/// or a passphrase, which there's no terminal to ask on.
fn no_prompts(repo: &Repo, command: &mut Command) {
    command
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            format!("{} -o BatchMode=yes", ssh_command(repo)),
        );
}

// ---------------------------------------------------------------- fetching

/// The remotes that local branches track, each once: what the background
/// fetch keeps up to date. Others are fetched by hand.
pub fn tracked_remotes(repo: &Repo) -> Vec<String> {
    let text = try_run(
        repo,
        &[
            "for-each-ref",
            "--format=%(upstream:remotename)",
            "refs/heads",
        ],
    );
    parse_tracked_remotes(&text.unwrap_or_default())
}

/// A branch with no upstream has an empty line, and one tracking a local
/// branch has `.`, neither of which is a remote.
fn parse_tracked_remotes(text: &str) -> Vec<String> {
    let mut remotes: Vec<String> = Vec::new();
    for remote in text
        .lines()
        .map(str::trim)
        .filter(|remote| !remote.is_empty() && *remote != ".")
    {
        if !remotes.iter().any(|known| known == remote) {
            remotes.push(remote.to_string());
        }
    }
    remotes
}

/// Fetches these remotes in the background, on a best-effort basis: it never
/// asks for anything, not even through Git Credential Manager's sign-in
/// window, so it only works where signing in already happened, as with an
/// SSH agent or stored credentials. It leaves out the housekeeping a fetch
/// may start, which can be slow, and FETCH_HEAD, which belongs to the user's
/// own fetches.
pub fn background_fetch(repo: &Repo, remotes: &[String]) -> Result<()> {
    if remotes.is_empty() {
        return Ok(());
    }
    let mut fetch = repo.command(&["-c", "gc.auto=0", "-c", "maintenance.auto=false"]);
    fetch
        .args(["fetch", "--quiet", "--no-write-fetch-head", "--multiple"])
        .args(remotes)
        .env("GCM_INTERACTIVE", "never");
    no_prompts(repo, &mut fetch);
    // Its first line is enough for the message `:fetch` shows.
    output(&mut fetch).map(|_| ()).map_err(|error| {
        let error = error.to_string();
        anyhow!(
            "{}",
            error.lines().next().unwrap_or("git fetch failed").trim()
        )
    })
}

/// How git runs SSH: the user's own setting if they have one.
fn ssh_command(repo: &Repo) -> String {
    std::env::var("GIT_SSH_COMMAND")
        .ok()
        .or_else(|| try_run(repo, &["config", "core.sshCommand"]))
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| "ssh".to_string())
}

// ---------------------------------------------------------------- staging

pub fn status(repo: &Repo) -> Result<Status> {
    let text = run(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
        ],
    )?;
    Ok(parse_status(&text))
}

/// Each entry looks like `XY path`, where X is the staged state and Y the
/// unstaged state. Entries are NUL separated, so paths need no unquoting.
fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    for entry in text.split('\0').filter(|e| e.len() > 3) {
        let x = entry.chars().next().unwrap_or(' ');
        let y = entry.chars().nth(1).unwrap_or(' ');
        let path = entry[3..].to_string();
        let conflicted = x == 'U' || y == 'U' || (x == y && (x == 'A' || x == 'D'));
        if conflicted {
            status.conflicted.push(FileEntry {
                status: 'U',
                path,
                ..Default::default()
            });
        } else if x == '?' {
            status.unstaged.push(FileEntry {
                status: '?',
                path,
                ..Default::default()
            });
        } else if x != '!' {
            if x != ' ' {
                status.staged.push(FileEntry {
                    status: x,
                    path: path.clone(),
                    ..Default::default()
                });
            }
            if y != ' ' {
                status.unstaged.push(FileEntry {
                    status: y,
                    path,
                    ..Default::default()
                });
            }
        }
    }
    status
}

pub fn stage(repo: &Repo, path: &str) -> Result<()> {
    run_ok(repo, &["add", "--", path])
}

/// Applies a patch to the index, leaving the working tree as it is: staging
/// part of a file, or with `reverse`, unstaging it. Git checks the patch
/// against the index first, so one made before the file changed fails, and
/// changes nothing.
pub fn apply_to_index(repo: &Repo, patch: &str, reverse: bool) -> Result<()> {
    apply(
        repo,
        patch,
        if reverse {
            &["--cached", "-R"]
        } else {
            &["--cached"]
        },
    )
}

/// Takes a patch back out of the working tree, leaving the index as it is:
/// discarding part of a file's changes. Git checks the patch against the
/// file first, so one made before the file changed fails, and changes
/// nothing.
pub fn unapply_from_working_tree(repo: &Repo, patch: &str) -> Result<()> {
    apply(repo, patch, &["-R"])
}

/// `git apply`, with the patch given on its input.
fn apply(repo: &Repo, patch: &str, options: &[&str]) -> Result<()> {
    let mut args = vec!["apply", "--whitespace=nowarn"];
    args.extend(options);
    args.push("-");
    run_with_input(repo, &args, patch.as_bytes()).map(|_| ())
}

/// Runs git with `input` on its standard input, returning what it printed,
/// as bytes, which may not be text.
fn run_with_input(repo: &Repo, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut command = repo.command(args);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| anyhow!("could not run git: {e}"))?;
    let mut stdin = child.stdin.take();
    // Written on a thread of its own, so a git that prints while it reads
    // can't block on a full pipe while gclic blocks on writing.
    let output = thread::scope(|scope| {
        scope.spawn(move || stdin.as_mut().map(|stdin| stdin.write_all(input)));
        child.wait_with_output()
    })?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}

// ------------------------------------------------------------------- undo

/// What the index holds for these paths, as `git ls-files --stage` prints
/// it, conflicts' stages included: enough to put them back exactly.
pub fn index_entries(repo: &Repo, paths: &[String]) -> Result<String> {
    let mut args = vec!["ls-files", "--stage", "-z", "--"];
    args.extend(paths.iter().map(String::as_str));
    run(repo, &args)
}

/// Puts these paths' index entries back as `index_entries` read them: each
/// path's entries go, then the saved ones come back.
pub fn set_index_entries(repo: &Repo, paths: &[String], entries: &str) -> Result<()> {
    let listed: String = paths.iter().map(|path| format!("{path}\0")).collect();
    run_with_input(
        repo,
        &["update-index", "-z", "--force-remove", "--stdin"],
        listed.as_bytes(),
    )?;
    if !entries.is_empty() {
        run_with_input(
            repo,
            &["update-index", "-z", "--index-info"],
            entries.as_bytes(),
        )?;
    }
    Ok(())
}

/// Files of the working tree kept in git's object store, so they can be put
/// back: each one's blob id, or None where there's no file. All in one git
/// call, as one per file adds up when discarding many.
pub fn save_files(repo: &Repo, paths: &[String]) -> Result<Vec<Option<String>>> {
    let present: Vec<&String> = paths
        .iter()
        .filter(|path| repo.file(path).is_file())
        .collect();
    // Git reads the paths a line each, which a name with a newline in it
    // would break, so those few go one at a time.
    let (listed, odd): (Vec<&String>, Vec<&String>) =
        present.into_iter().partition(|path| !path.contains('\n'));
    let mut blobs = std::collections::HashMap::new();
    if !listed.is_empty() {
        let input: String = listed.iter().map(|path| format!("{path}\n")).collect();
        let printed = run_with_input(
            repo,
            &["hash-object", "-w", "--no-filters", "--stdin-paths"],
            input.as_bytes(),
        )?;
        let printed = String::from_utf8_lossy(&printed);
        if printed.lines().count() != listed.len() {
            bail!("git hash-object didn't save every file");
        }
        blobs.extend(listed.into_iter().zip(printed.lines().map(str::to_string)));
    }
    for path in odd {
        blobs.insert(
            path,
            run(repo, &["hash-object", "-w", "--no-filters", "--", path])?
                .trim()
                .to_string(),
        );
    }
    Ok(paths.iter().map(|path| blobs.get(path).cloned()).collect())
}

/// Puts back a file `save_file` kept, or with None, removes it.
pub fn restore_file(repo: &Repo, path: &str, saved: Option<&str>) -> Result<()> {
    let file = repo.file(path);
    match saved {
        Some(blob) => {
            let bytes = run_with_input(repo, &["cat-file", "blob", blob], &[])?;
            if let Some(folder) = file.parent() {
                std::fs::create_dir_all(folder)?;
            }
            std::fs::write(&file, bytes)?;
        }
        None if file.exists() => std::fs::remove_file(&file)?,
        None => {}
    }
    Ok(())
}

pub fn stage_all(repo: &Repo) -> Result<()> {
    run_ok(repo, &["add", "-A"])
}

/// `reset` rather than `restore --staged` because it also works before the
/// first commit, when there is no HEAD to restore from.
pub fn unstage(repo: &Repo, path: &str) -> Result<()> {
    run_ok(repo, &["reset", "-q", "--", path])
}

pub fn unstage_all(repo: &Repo) -> Result<()> {
    run_ok(repo, &["reset", "-q"])
}

pub fn restore(repo: &Repo, path: &str) -> Result<()> {
    run_ok(repo, &["restore", "--", path])
}

pub fn delete_untracked(repo: &Repo, path: &str) -> Result<()> {
    run_ok(repo, &["clean", "-f", "--", path])
}

/// Restores these changed files and deletes these untracked ones, each kind
/// in one git call.
pub fn discard_all(repo: &Repo, restore: &[String], delete: &[String]) -> Result<()> {
    let with_paths = |command: &[&str], paths: &[String]| {
        let mut args: Vec<&str> = command.to_vec();
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
        run_ok(repo, &args)
    };
    if !restore.is_empty() {
        with_paths(&["restore"], restore)?;
    }
    if !delete.is_empty() {
        with_paths(&["clean", "-f"], delete)?;
    }
    Ok(())
}

pub fn commit(repo: &Repo, message: &str) -> Result<()> {
    run_ok(repo, &["commit", "-m", message])
}

// ------------------------------------------------------------------ diffs

/// The diff of the whole file, or just the changes.
pub fn diff(repo: &Repo, source: &DiffSource, whole: bool) -> Result<String> {
    const EXT: &str = "--no-ext-diff";
    const COLOR: &str = "--no-color";
    // So much context that a hunk takes in the whole file.
    let context = if whole {
        "--unified=1000000"
    } else {
        "--unified=3"
    };
    match source {
        DiffSource::Unstaged(path) => run(repo, &["diff", EXT, COLOR, context, "--", path]),
        DiffSource::Staged(path) => {
            run(repo, &["diff", EXT, COLOR, context, "--cached", "--", path])
        }
        DiffSource::Untracked(path) => {
            // --no-index exits with 1 when the files differ, which here is
            // always, so the exit code can't be treated as failure.
            let output = repo
                .command(&["diff", EXT, COLOR, "--no-index", "--", "/dev/null", path])
                .output()
                .map_err(|e| anyhow!("could not run git: {e}"))?;
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        // Not a diff: the file itself, whose conflict markers hold both sides.
        // Empty if one side deleted it.
        DiffSource::Conflict(path) => Ok(std::fs::read(repo.file(path))
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()),
        DiffSource::Commit { sha, path } => run(
            repo,
            &[
                "show",
                EXT,
                COLOR,
                "--format=",
                "--diff-merges=first-parent",
                context,
                sha,
                "--",
                path,
            ],
        ),
        DiffSource::Refs { a, b, path } => {
            run(repo, &["diff", EXT, COLOR, context, a, b, "--", path])
        }
        DiffSource::Stash {
            sha,
            path,
            untracked,
        } => {
            let (old, new) = stash_sides(sha, *untracked);
            run(repo, &["diff", EXT, COLOR, context, &old, &new, "--", path])
        }
    }
}

/// Git's empty tree, which an untracked file in a stash is new against.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// What a stashed file goes from and to: its tracked changes are against the
/// stash's first parent, the commit it was made on, and its untracked files
/// are in its third parent, against nothing.
fn stash_sides(sha: &str, untracked: bool) -> (String, String) {
    if untracked {
        (EMPTY_TREE.to_string(), format!("{sha}^3"))
    } else {
        (format!("{sha}^1"), sha.to_string())
    }
}

/// The whole file on each side of a diff, old then new. None for a side where
/// the file doesn't exist, or can't be read.
/// Both versions are read at once, as each is a git call of its own.
pub fn file_versions(repo: &Repo, source: &DiffSource) -> (Option<String>, Option<String>) {
    let path = source.path();
    let (old, new) = match source {
        // An empty revision makes `:path`, the file as staged in the index.
        DiffSource::Unstaged(_) => (Version::At(String::new()), Version::OnDisk),
        DiffSource::Staged(_) => (Version::At("HEAD".to_string()), Version::At(String::new())),
        DiffSource::Untracked(_) => (Version::Missing, Version::OnDisk),
        // Both sides are lines of the file as it is, markers and all.
        DiffSource::Conflict(_) => (Version::OnDisk, Version::OnDisk),
        // The diff is against the first parent, which `^` names.
        DiffSource::Commit { sha, .. } => {
            (Version::At(format!("{sha}^")), Version::At(sha.clone()))
        }
        DiffSource::Refs { a, b, .. } => (Version::At(a.clone()), Version::At(b.clone())),
        DiffSource::Stash { sha, untracked, .. } => {
            let (from, to) = stash_sides(sha, *untracked);
            // Untracked files come from nothing, which has no file to read.
            let from = if from == EMPTY_TREE {
                Version::Missing
            } else {
                Version::At(from)
            };
            (from, Version::At(to))
        }
    };
    thread::scope(|scope| {
        let old = scope.spawn(|| old.read(repo, path));
        let new = new.read(repo, path);
        (old.join().unwrap_or(None), new)
    })
}

/// Where to read one side of a diff from.
enum Version {
    At(String),
    OnDisk,
    Missing,
}

impl Version {
    fn read(&self, repo: &Repo, path: &str) -> Option<String> {
        match self {
            Version::At(revision) => run(repo, &["show", &format!("{revision}:{path}")]).ok(),
            Version::OnDisk => {
                let bytes = std::fs::read(repo.file(path)).ok()?;
                Some(String::from_utf8_lossy(&bytes).into_owned())
            }
            Version::Missing => None,
        }
    }
}

/// Parses `--name-status -z` output: `M\0path\0A\0other\0`.
fn parse_name_status(text: &str) -> Vec<FileEntry> {
    let mut files = Vec::new();
    let mut fields = text.split('\0');
    while let (Some(letter), Some(path)) = (fields.next(), fields.next()) {
        files.push(FileEntry {
            status: letter.chars().next().unwrap_or('?'),
            path: path.to_string(),
            ..Default::default()
        });
    }
    files
}

// ------------------------------------------------------------------ graph

/// Commits reachable from `refs`, newest first. `--topo-order` guarantees that
/// children come before their parents, which the graph layout relies on.
pub fn log(repo: &Repo, refs: &[String], limit: usize) -> Result<Vec<Commit>> {
    let limit = format!("-n{limit}");
    let mut args = vec![
        "log",
        "--topo-order",
        limit.as_str(),
        "--format=%H%x00%P%x00%an%x00%s",
    ];
    args.extend(refs.iter().map(String::as_str));
    args.push("--");
    Ok(parse_log(&run(repo, &args)?))
}

fn parse_log(text: &str) -> Vec<Commit> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\0');
            Some(Commit {
                sha: fields.next()?.to_string(),
                parents: fields
                    .next()?
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
                author: fields.next()?.to_string(),
                subject: fields.next()?.to_string(),
            })
        })
        .collect()
}

/// The commits reachable from `refs` that aren't on any remote-tracking branch.
pub fn unpushed(repo: &Repo, refs: &[String]) -> Result<HashSet<String>> {
    let mut args = vec!["rev-list"];
    args.extend(refs.iter().map(String::as_str));
    args.extend(["--not", "--remotes", "--"]);
    Ok(run(repo, &args)?.lines().map(str::to_string).collect())
}

/// Changes whenever a ref or HEAD moves, for spotting commits, checkouts and
/// fetches made outside the app without reloading the whole graph. HEAD names
/// the checked-out branch, whose commit is among the refs, or a commit itself.
pub fn refs_fingerprint(repo: &Repo) -> String {
    let refs = try_run(repo, &["for-each-ref", "--format=%(objectname) %(refname)"]);
    format!(
        "{}\n{}",
        refs.unwrap_or_default(),
        read_head(repo).unwrap_or_default()
    )
}

/// What HEAD holds: `ref: refs/heads/main`, or a commit's sha when detached.
fn read_head(repo: &Repo) -> Option<String> {
    let head = std::fs::read_to_string(repo.git_dir.join("HEAD")).ok()?;
    Some(head.trim().to_string())
}

/// A merge, rebase, cherry-pick or revert that stopped halfway, usually on
/// conflicts, as the files git leaves in its directory while one is going.
pub fn operation_in_progress(repo: &Repo) -> Option<&'static str> {
    let dir = &repo.git_dir;
    let operations = [
        ("MERGE_HEAD", "merging"),
        ("rebase-merge", "rebasing"),
        ("rebase-apply", "rebasing"),
        ("CHERRY_PICK_HEAD", "cherry-picking"),
        ("REVERT_HEAD", "reverting"),
    ];
    operations
        .iter()
        .find(|(file, _)| dir.join(file).exists())
        .map(|(_, name)| *name)
}

/// Where a branch is checked out, if that's another worktree than this one:
/// `../name` when it's next to this one, as worktrees usually are, or else the
/// whole path. None for this worktree.
pub fn other_worktree(repo: &Repo, path: &str) -> Option<String> {
    let here = repo.root.canonicalize().ok()?;
    let there = Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(path));
    if there == here {
        return None;
    }
    match (there.parent(), here.parent(), there.file_name()) {
        (Some(a), Some(b), Some(name)) if a == b => Some(format!("../{}", name.to_string_lossy())),
        _ => Some(path.to_string()),
    }
}

/// The checked-out branch. None on a detached HEAD.
pub fn current_branch(repo: &Repo) -> Option<String> {
    read_head(repo)?
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

/// The default graph view: the current branch and its remote counterpart,
/// which is its upstream or else origin's branch of the same name. Refs that
/// don't exist, like that one or a branch with no commits yet, are dropped
/// later, when `resolve_all` finds nothing for them.
pub fn default_refs(repo: &Repo) -> Vec<String> {
    let Some(branch) = current_branch(repo) else {
        return vec!["HEAD".to_string()]; // detached HEAD
    };
    let upstream = try_run(
        repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    );
    let remote = upstream.unwrap_or_else(|| format!("origin/{branch}"));
    vec![branch, remote]
}

/// The commit each ref points at, in the same order, with None for those that
/// don't exist. One git call does them all, unless one of them doesn't exist,
/// which fails it: then they're asked for one by one.
pub fn resolve_all(repo: &Repo, refs: &[String]) -> Vec<Option<String>> {
    // A ref starting with a dash would be read as a flag.
    if !refs.is_empty() && refs.iter().all(|reference| !reference.starts_with('-')) {
        let commits: Vec<String> = refs.iter().map(|r| format!("{r}^{{commit}}")).collect();
        let mut args = vec!["rev-parse"];
        args.extend(commits.iter().map(String::as_str));
        if let Ok(text) = run(repo, &args) {
            let shas: Vec<Option<String>> = text.lines().map(|sha| Some(sha.to_string())).collect();
            if shas.len() == refs.len() {
                return shas;
            }
        }
    }
    refs.iter()
        .map(|reference| resolve(repo, reference))
        .collect()
}

/// A sha as people write it: its first seven characters.
pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// The commit sha a ref points at, if it exists.
pub fn resolve(repo: &Repo, reference: &str) -> Option<String> {
    if reference.starts_with('-') {
        return None; // would be read as a flag
    }
    let commit = format!("{reference}^{{commit}}");
    try_run(repo, &["rev-parse", "--verify", "-q", commit.as_str()])
}

/// Whether this could be a commit sha, full or abbreviated as git usually does.
pub fn looks_like_sha(text: &str) -> bool {
    (7..=40).contains(&text.len()) && text.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// A ref as worth showing on screen: shas get shortened, names stay as they are.
pub fn display_ref(repo: &Repo, reference: &str) -> String {
    if looks_like_sha(reference) {
        let commit = format!("{reference}^{{commit}}");
        if let Some(short) = try_run(repo, &["rev-parse", "--short", commit.as_str()]) {
            return short;
        }
    }
    reference.to_string()
}

/// A one-line description of a commit: its short sha, two spaces, its subject.
pub fn commit_label(repo: &Repo, sha: &str) -> Option<String> {
    try_run(repo, &["show", "-s", "--format=%h  %s", sha])
}

pub fn commit_details(repo: &Repo, sha: &str) -> Result<CommitDetails> {
    // The message ends in a NUL, as it can hold anything else, and the
    // change's size follows it, in the same call.
    let text = run(
        repo,
        &[
            "show",
            "--shortstat",
            "--diff-merges=first-parent",
            "--date=iso",
            "--format=%H%n%an <%ae>%n%ad%n%B%x00",
            sha,
        ],
    )?;
    let (header, stat) = text.split_once('\0').unwrap_or((&text, ""));
    let mut lines = header.splitn(4, '\n');
    let (files, lines_changed) = parse_shortstat(stat);
    Ok(CommitDetails {
        sha: lines.next().unwrap_or("").to_string(),
        author: lines.next().unwrap_or("").to_string(),
        date: lines.next().unwrap_or("").to_string(),
        message: lines.next().unwrap_or("").trim_end().to_string(),
        files,
        lines: lines_changed,
    })
}

/// The files and lines in git's `--shortstat`: ` 3 files changed, 42
/// insertions(+), 7 deletions(-)`, either count left out when it's 0, and
/// nothing at all for a commit that changes nothing.
fn parse_shortstat(stat: &str) -> (usize, Lines) {
    let (mut files, mut added, mut removed) = (0, 0, 0);
    for part in stat.trim().split(", ") {
        let Some((count, what)) = part.split_once(' ') else {
            continue;
        };
        let Ok(count) = count.parse() else {
            continue;
        };
        if what.starts_with("file") {
            files = count;
        } else if what.starts_with("insertion") {
            added = count;
        } else if what.starts_with("deletion") {
            removed = count;
        }
    }
    (files, Lines::Counted { added, removed })
}

pub fn commit_files(repo: &Repo, sha: &str) -> Result<Vec<FileEntry>> {
    let show = [
        "show",
        "--format=",
        "--no-renames",
        "--diff-merges=first-parent",
        "-z",
        sha,
    ];
    files_with_lines(repo, &show)
}

/// The files a diff command lists, with how many lines each adds and
/// removes. Git won't give both in one go, so it's asked twice, at once.
fn files_with_lines(repo: &Repo, diff: &[&str]) -> Result<Vec<FileEntry>> {
    // Right after the subcommand, as a diff's arguments can end with `--`.
    let with = |option: &'static str| [&diff[..1], &[option], &diff[1..]].concat();
    let (files, numstat) = thread::scope(|scope| {
        let numstat = scope.spawn(|| run(repo, &with("--numstat")));
        (run(repo, &with("--name-status")), numstat.join())
    });
    let mut files = parse_name_status(&files?);
    if let Ok(Ok(numstat)) = numstat {
        add_lines(&mut files, &numstat);
    }
    Ok(files)
}

/// Fills in each file's lines from `--numstat -z` output: `3\t1\tpath\0`, with
/// `-` for both counts on a binary file.
fn add_lines(files: &mut [FileEntry], numstat: &str) {
    let mut counts = std::collections::HashMap::new();
    for entry in numstat.split('\0').filter(|entry| !entry.is_empty()) {
        let mut fields = entry.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let lines = match (added.parse(), removed.parse()) {
            (Ok(added), Ok(removed)) => Lines::Counted { added, removed },
            _ => Lines::Binary,
        };
        counts.insert(path, lines);
    }
    for file in files {
        if let Some(lines) = counts.get(file.path.as_str()) {
            file.lines = *lines;
        }
    }
}

// ----------------------------------------------------------------- stashes

/// Work put aside with `git stash`. Under the hood, a commit whose first
/// parent is the commit it was made on, with the untracked files, if any, in
/// a third parent.
#[derive(Debug, Clone, PartialEq)]
pub struct Stash {
    pub sha: String,
    /// The commit it was made on, which its changes are against.
    pub base: String,
    pub has_untracked: bool,
    /// As git names it now: `stash@{0}` is the newest, and the numbers shift
    /// as stashes come and go, so actions find it again by its sha.
    pub name: String,
    /// "WIP on main: …", or "On main: " and the message it was given.
    pub message: String,
    /// "2 hours ago".
    pub age: String,
}

/// The stashes, newest first.
pub fn stashes(repo: &Repo) -> Result<Vec<Stash>> {
    Ok(parse_stashes(&run(
        repo,
        &["stash", "list", "--format=%H%x00%P%x00%gd%x00%gs%x00%cr"],
    )?))
}

fn parse_stashes(text: &str) -> Vec<Stash> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(5, '\0');
            let sha = fields.next()?.to_string();
            let parents: Vec<&str> = fields.next()?.split_whitespace().collect();
            Some(Stash {
                sha,
                base: parents.first()?.to_string(),
                has_untracked: parents.len() > 2,
                name: fields.next()?.to_string(),
                message: fields.next()?.to_string(),
                age: fields.next()?.to_string(),
            })
        })
        .collect()
}

/// A stash's files: the tracked ones it changes, and its untracked ones,
/// marked `?` as the Changes pane marks them.
pub fn stash_files(repo: &Repo, stash: &Stash) -> Result<Vec<FileEntry>> {
    // A stash is a merge, which commit_files shows against its first parent.
    let mut files = commit_files(repo, &stash.sha)?;
    if stash.has_untracked {
        // Its untracked files, as added to nothing.
        let untracked = format!("{}^3", stash.sha);
        let mut added = files_with_lines(
            repo,
            &["diff", "--no-renames", "-z", EMPTY_TREE, &untracked, "--"],
        )?;
        for file in &mut added {
            file.status = '?';
        }
        files.extend(added);
    }
    Ok(files)
}

/// Puts a stash's changes back, all unstaged, keeping the stash.
pub fn stash_apply(repo: &Repo, stash: &Stash) -> Result<()> {
    run_ok(repo, &["stash", "apply", &stash_name(repo, stash)?])
}

/// Applies a stash and, if that worked, drops it. On a conflict, git keeps it.
pub fn stash_pop(repo: &Repo, stash: &Stash) -> Result<()> {
    run_ok(repo, &["stash", "pop", &stash_name(repo, stash)?])
}

pub fn stash_drop(repo: &Repo, stash: &Stash) -> Result<()> {
    run_ok(repo, &["stash", "drop", &stash_name(repo, stash)?])
}

/// The stash's name now, which pop and drop need, as a sha won't do for them.
/// Found by its sha, as names shift when another stash comes or goes.
fn stash_name(repo: &Repo, stash: &Stash) -> Result<String> {
    let now = stashes(repo)?.into_iter().find(|now| now.sha == stash.sha);
    now.map(|now| now.name)
        .ok_or_else(|| anyhow!("{} is gone: it was dropped or popped", stash.name))
}

// --------------------------------------------------------------- branches

/// Local branches first, then remote ones.
pub fn branches(repo: &Repo) -> Result<Vec<Branch>> {
    let text = run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(refname:lstrip=3)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(worktreepath)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    Ok(parse_branches(&text))
}

fn parse_branches(text: &str) -> Vec<Branch> {
    let mut branches = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [full, short, without_remote, upstream, track, worktree] = fields[..] else {
            continue;
        };
        if full.starts_with("refs/remotes/") {
            // `origin/HEAD` is only a pointer to the remote's default branch.
            if without_remote != "HEAD" {
                branches.push(Branch {
                    name: short.to_string(),
                    local_name: without_remote.to_string(),
                    is_remote: true,
                    upstream: None,
                    ahead_behind: None,
                    worktree: None,
                });
            }
        } else {
            branches.push(Branch {
                name: short.to_string(),
                local_name: short.to_string(),
                is_remote: false,
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
                ahead_behind: if upstream.is_empty() {
                    None
                } else {
                    parse_track(track)
                },
                worktree: (!worktree.is_empty()).then(|| worktree.to_string()),
            });
        }
    }
    branches
}

/// Reads `%(upstream:track,nobracket)`: empty when in sync, `ahead 2`,
/// `behind 1`, `ahead 2, behind 1`, or `gone` when the upstream was deleted.
fn parse_track(track: &str) -> Option<(usize, usize)> {
    if track == "gone" {
        return None;
    }
    let count = |word: &str| -> usize {
        track
            .split(", ")
            .find_map(|part| part.strip_prefix(word)?.trim().parse().ok())
            .unwrap_or(0)
    };
    Some((count("ahead "), count("behind ")))
}

/// The branches, local and remote, whose history holds this commit, by
/// their short names. None if `sha` isn't a commit. A remote's HEAD, only a
/// pointer to one of its branches, is left out, as in `branches`.
pub fn branches_containing(repo: &Repo, sha: &str) -> Option<Vec<String>> {
    let commit = resolve(repo, sha)?;
    let text = try_run(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)",
            "--contains",
            commit.as_str(),
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let is_remote_head = |full: &str| full.starts_with("refs/remotes/") && full.ends_with("/HEAD");
    let names = text.lines().filter_map(|line| line.split_once('\0'));
    Some(
        names
            .filter(|(full, _)| !is_remote_head(full))
            .map(|(_, short)| short.to_string())
            .collect(),
    )
}

pub fn tags(repo: &Repo) -> Result<Vec<String>> {
    let text = run(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/tags"],
    )?;
    Ok(text.lines().map(str::to_string).collect())
}

pub fn switch(repo: &Repo, local_name: &str) -> Result<()> {
    run_ok(repo, &["switch", local_name])
}

/// Creates a local branch tracking this remote one, and switches to it.
pub fn switch_track(repo: &Repo, remote_name: &str) -> Result<()> {
    run_ok(repo, &["switch", "--track", remote_name])
}

// ---------------------------------------------------------------- compare

/// How many commits `a` has that `b` doesn't, and the other way around.
pub fn ahead_behind(repo: &Repo, a: &str, b: &str) -> Result<(usize, usize)> {
    let range = format!("{a}...{b}");
    let text = run(
        repo,
        &["rev-list", "--left-right", "--count", range.as_str(), "--"],
    )?;
    let mut counts = text
        .split_whitespace()
        .map(|n| n.parse::<usize>().unwrap_or(0));
    Ok((counts.next().unwrap_or(0), counts.next().unwrap_or(0)))
}

pub fn merge_base(repo: &Repo, a: &str, b: &str) -> Option<String> {
    try_run(repo, &["merge-base", a, b])
}

pub fn changed_files(repo: &Repo, a: &str, b: &str) -> Result<Vec<FileEntry>> {
    files_with_lines(repo, &["diff", "--no-renames", "-z", a, b, "--"])
}

#[cfg(test)]
mod repo_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_repo::TestRepo;

    fn entry(status: char, path: &str) -> FileEntry {
        FileEntry {
            status,
            path: path.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn shortstat_reads_files_and_lines_either_left_out() {
        let counted = |added, removed| Lines::Counted { added, removed };
        let stat = " 3 files changed, 42 insertions(+), 7 deletions(-)\n";
        assert_eq!(parse_shortstat(stat), (3, counted(42, 7)));
        assert_eq!(
            parse_shortstat(" 1 file changed, 1 deletion(-)"),
            (1, counted(0, 1))
        );
        assert_eq!(parse_shortstat(""), (0, counted(0, 0)));
    }

    #[test]
    fn status_splits_entries_into_staged_and_unstaged() {
        let status = parse_status("M  staged.rs\0 M unstaged.rs\0MM both.rs\0?? new file.rs\0");
        assert_eq!(
            status.staged,
            vec![entry('M', "staged.rs"), entry('M', "both.rs")]
        );
        assert_eq!(
            status.unstaged,
            vec![
                entry('M', "unstaged.rs"),
                entry('M', "both.rs"),
                entry('?', "new file.rs")
            ]
        );
    }

    #[test]
    fn shas_are_recognised_by_length_and_digits() {
        assert!(looks_like_sha("abc1234"));
        assert!(looks_like_sha("9AFC3E9A7D497E015C3C05968EA7D3FEB403770F"));
        assert!(!looks_like_sha("abc123")); // too short to be sure
        assert!(!looks_like_sha(&"a".repeat(41)));
        assert!(!looks_like_sha("feature"));
        assert!(!looks_like_sha(""));
    }

    #[test]
    fn status_keeps_merge_conflicts_apart() {
        let status = parse_status("UU a\0AA b\0DD c\0 M d\0");
        assert!(status.staged.is_empty());
        assert_eq!(
            status.conflicted,
            vec![entry('U', "a"), entry('U', "b"), entry('U', "c")]
        );
        assert_eq!(status.unstaged, vec![entry('M', "d")]);
    }

    #[test]
    fn tracked_remotes_come_once_each_and_only_remotes() {
        let text = "origin\n\norigin\nupstream\n.\n";
        assert_eq!(parse_tracked_remotes(text), ["origin", "upstream"]);
    }

    #[test]
    fn stashes_read_their_base_and_whether_they_hold_untracked_files() {
        let text = "s1\0base1 index1 untracked1\0stash@{0}\0WIP on main: abc fix\x002 hours ago\n\
                    s2\0base2 index2\0stash@{1}\0On dev: try this\x003 days ago\n";
        let stashes = parse_stashes(text);
        assert_eq!(stashes.len(), 2);
        assert_eq!(
            (stashes[0].base.as_str(), stashes[0].has_untracked),
            ("base1", true)
        );
        assert_eq!(
            (stashes[1].name.as_str(), stashes[1].has_untracked),
            ("stash@{1}", false)
        );
        assert_eq!(stashes[1].message, "On dev: try this");
        assert_eq!(stashes[0].age, "2 hours ago");
    }

    #[test]
    fn name_status_reads_letter_path_pairs() {
        let files = parse_name_status("M\0src/a.rs\0D\0b c.rs\0");
        assert_eq!(files, vec![entry('M', "src/a.rs"), entry('D', "b c.rs")]);
    }

    #[test]
    fn log_reads_parents_and_subject() {
        let commits = parse_log("aaa\0bbb ccc\0Ann\0merge things\nbbb\0\0Bo\0root\n");
        assert_eq!(commits[0].parents, vec!["bbb", "ccc"]);
        assert_eq!(commits[0].author, "Ann");
        assert_eq!(commits[0].subject, "merge things");
        assert!(commits[1].parents.is_empty());
    }

    #[test]
    fn upstream_tracking_reads_every_form_git_prints() {
        assert_eq!(parse_track(""), Some((0, 0)));
        assert_eq!(parse_track("ahead 2"), Some((2, 0)));
        assert_eq!(parse_track("behind 13"), Some((0, 13)));
        assert_eq!(parse_track("ahead 2, behind 1"), Some((2, 1)));
        assert_eq!(parse_track("gone"), None);
    }

    #[test]
    fn branches_skip_remote_head_and_strip_the_remote() {
        let text = "refs/heads/main\0main\0\0origin/main\0ahead 2\0/repos/main\n\
                    refs/remotes/origin/HEAD\0origin\0HEAD\0\0\0\n\
                    refs/remotes/origin/a/b\0origin/a/b\0a/b\0\0\0\n";
        let branches = parse_branches(text);
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].local_name, "main");
        assert_eq!(branches[1].name, "origin/a/b");
        assert_eq!(branches[1].local_name, "a/b");
        assert_eq!(branches[0].upstream.as_deref(), Some("origin/main"));
        assert_eq!(branches[0].ahead_behind, Some((2, 0)));
        assert_eq!(branches[1].ahead_behind, None);
        assert_eq!(branches[0].worktree.as_deref(), Some("/repos/main"));
        assert_eq!(branches[1].worktree, None);
        assert!(!branches[0].is_remote);
        assert!(branches[1].is_remote);
    }

    // These run on repos made for them, comparing the quick answers with
    // what git itself says.

    #[test]
    fn a_repo_is_found_from_any_folder_inside_it() {
        let made = TestRepo::new();
        made.write("deep/er/file", "");
        assert_eq!(
            Repo::discover(&made.file("deep/er")).as_ref(),
            Some(&made.repo)
        );
        assert!(made.git_dir.ends_with(".git"));
        assert_eq!(
            Repo::discover(&std::env::temp_dir().join("gclic-no-such-folder")),
            None
        );
    }

    #[test]
    fn current_branch_agrees_with_git() {
        let repo = TestRepo::with_history();
        let from_git = try_run(&repo, &["symbolic-ref", "--short", "-q", "HEAD"]);
        assert_eq!(current_branch(&repo), from_git);
        assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    }

    #[test]
    fn resolving_several_refs_at_once_matches_one_at_a_time() {
        let repo = TestRepo::with_history();
        let refs = [
            "HEAD".to_string(),
            "HEAD~1".to_string(),
            "feature".to_string(),
        ];
        let expected: Vec<_> = refs.iter().map(|r| resolve(&repo, r)).collect();
        assert!(expected.iter().all(Option::is_some));
        assert_eq!(resolve_all(&repo, &refs), expected);
        assert_eq!(expected[1].as_deref(), Some(repo.sha("HEAD~1").as_str()));
    }

    #[test]
    fn a_missing_ref_resolves_to_nothing_without_losing_the_others() {
        let repo = TestRepo::with_history();
        let refs = [
            "HEAD".to_string(),
            "no-such-branch-here".to_string(),
            "-x".to_string(),
        ];
        let shas = resolve_all(&repo, &refs);
        assert_eq!(shas[0], Some(repo.sha("HEAD")));
        assert_eq!(shas[1..], [None, None]);
    }

    #[test]
    fn a_whole_file_diff_holds_every_line_of_the_file() {
        // The first commit's src/main.rs was edited later, so a diff against
        // it has changes, and unchanged lines that only a whole-file diff shows.
        let repo = TestRepo::with_history();
        let source = DiffSource::Refs {
            a: repo.sha("main~2"),
            b: "HEAD".to_string(),
            path: "src/main.rs".to_string(),
        };
        let changes = diff(&repo, &source, false).unwrap();
        let whole = diff(&repo, &source, true).unwrap();
        let lines_now = repo.read("src/main.rs").unwrap().lines().count();
        let shown = |diff: &str| {
            let new_side =
                |l: &&str| l.starts_with(' ') || (l.starts_with('+') && !l.starts_with("+++"));
            diff.lines().filter(new_side).count()
        };
        assert_eq!(shown(&whole), lines_now);
        assert!(shown(&changes) <= shown(&whole));
    }

    #[test]
    fn both_versions_of_a_file_come_back() {
        // The second commit changes src/main.rs.
        let repo = TestRepo::with_history();
        let source = DiffSource::Commit {
            sha: repo.sha("main~1"),
            path: "src/main.rs".to_string(),
        };
        let (old, new) = file_versions(&repo, &source);
        assert_eq!(
            old.as_deref(),
            Some("fn main() {\n    println!(\"one\");\n}\n")
        );
        assert!(new.is_some_and(|text| text.contains("let n = 2;")));
    }

    #[test]
    fn a_commit_s_files_come_with_their_lines() {
        let repo = TestRepo::with_history();
        let files = commit_files(&repo, "HEAD").unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "README.md");
        assert_eq!(
            files[0].lines,
            Lines::Counted {
                added: 2,
                removed: 0
            }
        );
    }

    #[test]
    fn numstat_counts_lines_and_marks_binary_files() {
        let mut files = vec![
            entry('M', "a.rs"),
            entry('A', "logo.png"),
            entry('M', "b.rs"),
        ];
        add_lines(&mut files, "3\t1\ta.rs\0-\t-\tlogo.png\0");
        assert_eq!(
            files[0].lines,
            Lines::Counted {
                added: 3,
                removed: 1
            }
        );
        assert_eq!(files[1].lines, Lines::Binary);
        assert_eq!(files[2].lines, Lines::Unknown);
    }
}
