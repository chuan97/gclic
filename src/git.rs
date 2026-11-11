//! Every interaction with git lives here. The rest of the app never sees a
//! subprocess or raw git output, only the typed data returned by this module.

use anyhow::{Result, anyhow, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    /// Status letter: M, A, D, ... plus `?` for untracked and `U` for conflicts.
    pub status: char,
    pub path: String,
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
    Commit { sha: String, path: String },
    Refs { a: String, b: String, path: String },
}

impl DiffSource {
    /// Whether this comes from the Changes pane, rather than from history.
    pub fn is_working_tree(&self) -> bool {
        matches!(
            self,
            DiffSource::Unstaged(_) | DiffSource::Staged(_) | DiffSource::Untracked(_) | DiffSource::Conflict(_)
        )
    }

    /// The file's path, relative to the repo root.
    pub fn path(&self) -> &str {
        match self {
            DiffSource::Unstaged(path)
            | DiffSource::Staged(path)
            | DiffSource::Untracked(path)
            | DiffSource::Conflict(path) => path,
            DiffSource::Commit { path, .. } | DiffSource::Refs { path, .. } => path,
        }
    }
}

fn git(args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        // Read-only refreshes shouldn't take the index lock and get in the
        // way of git commands the user runs in another terminal.
        .env("GIT_OPTIONAL_LOCKS", "0")
        // Nothing may wait for input on the terminal the TUI is drawing on.
        .stdin(Stdio::null())
        .output()
        .map_err(|e| anyhow!("could not run git: {e}"))
}

fn run(args: &[&str]) -> Result<String> {
    let output = git(args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Most failures explain themselves on stderr, but `git commit` reports
        // things like "nothing to commit" on stdout.
        let text = if stderr.trim().is_empty() { stdout } else { stderr };
        bail!("{}", text.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Like `run`, but returns None instead of an error when the command fails.
/// For questions where failing is a normal answer, like "is there an upstream?".
fn try_run(args: &[&str]) -> Option<String> {
    run(args).ok().map(|text| text.trim().to_string())
}

/// A git command the user typed on gclic's command line, set up to run
/// without a terminal: the TUI is drawing on it. Anything that would ask for
/// something fails at once instead of waiting forever: the editor, passwords,
/// and SSH passphrases. Keys in an SSH agent still work. Unlike gclic's own
/// reads, it takes locks as git normally does.
pub fn user_command(args: &[String]) -> Command {
    let mut command = Command::new("git");
    command
        // Git leaves colour out when its output isn't a terminal, and gclic
        // shows it, as the user would see it in one.
        .args(["-c", "color.ui=always"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        // A command with a space runs through git's shell, on every platform.
        // Git adds the file to edit after it, which the # makes a comment.
        .env("GIT_EDITOR", "exit 1 #")
        .env("GIT_SEQUENCE_EDITOR", "exit 1 #")
        .env("GIT_PAGER", "cat")
        .env("GIT_SSH_COMMAND", format!("{} -o BatchMode=yes", ssh_command()));
    command
}

/// How git runs SSH: the user's own setting if they have one.
fn ssh_command() -> String {
    std::env::var("GIT_SSH_COMMAND")
        .ok()
        .or_else(|| try_run(&["config", "core.sshCommand"]))
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| "ssh".to_string())
}

pub fn repo_root() -> Result<String> {
    Ok(run(&["rev-parse", "--show-toplevel"])?.trim().to_string())
}

// ---------------------------------------------------------------- staging

pub fn status() -> Result<Status> {
    let text = run(&[
        "status",
        "--porcelain=v1",
        "-z",
        "--no-renames",
        "--untracked-files=all",
    ])?;
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
            status.conflicted.push(FileEntry { status: 'U', path });
        } else if x == '?' {
            status.unstaged.push(FileEntry { status: '?', path });
        } else if x != '!' {
            if x != ' ' {
                status.staged.push(FileEntry { status: x, path: path.clone() });
            }
            if y != ' ' {
                status.unstaged.push(FileEntry { status: y, path });
            }
        }
    }
    status
}

pub fn stage(path: &str) -> Result<()> {
    run(&["add", "--", path]).map(|_| ())
}

pub fn stage_all() -> Result<()> {
    run(&["add", "-A"]).map(|_| ())
}

/// `reset` rather than `restore --staged` because it also works before the
/// first commit, when there is no HEAD to restore from.
pub fn unstage(path: &str) -> Result<()> {
    run(&["reset", "-q", "--", path]).map(|_| ())
}

pub fn unstage_all() -> Result<()> {
    run(&["reset", "-q"]).map(|_| ())
}

pub fn restore(path: &str) -> Result<()> {
    run(&["restore", "--", path]).map(|_| ())
}

pub fn delete_untracked(path: &str) -> Result<()> {
    run(&["clean", "-f", "--", path]).map(|_| ())
}

pub fn commit(message: &str) -> Result<()> {
    run(&["commit", "-m", message]).map(|_| ())
}

// ------------------------------------------------------------------ diffs

/// Whether diffs show whole files rather than just the changes. One switch for
/// the whole app, read by the UI and the worker thread alike, so the diffs
/// they each fetch always agree.
static WHOLE_FILES: AtomicBool = AtomicBool::new(false);

pub fn whole_files() -> bool {
    WHOLE_FILES.load(Ordering::Relaxed)
}

pub fn set_whole_files(on: bool) {
    WHOLE_FILES.store(on, Ordering::Relaxed);
}

pub fn diff(source: &DiffSource) -> Result<String> {
    diff_with(source, whole_files())
}

/// The diff of the whole file, or just the changes, whichever is showing.
pub fn diff_with(source: &DiffSource, whole: bool) -> Result<String> {
    const EXT: &str = "--no-ext-diff";
    const COLOR: &str = "--no-color";
    // So much context that a hunk takes in the whole file.
    let context = if whole { "--unified=1000000" } else { "--unified=3" };
    match source {
        DiffSource::Unstaged(path) => run(&["diff", EXT, COLOR, context, "--", path]),
        DiffSource::Staged(path) => run(&["diff", EXT, COLOR, context, "--cached", "--", path]),
        DiffSource::Untracked(path) => {
            // --no-index exits with 1 when the files differ, which here is
            // always, so the exit code can't be treated as failure.
            let output = git(&["diff", EXT, COLOR, "--no-index", "--", "/dev/null", path])?;
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        // Not a diff: the file itself, whose conflict markers hold both sides.
        // Empty if one side deleted it.
        DiffSource::Conflict(path) => Ok(std::fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default()),
        DiffSource::Commit { sha, path } => run(&[
            "show",
            EXT,
            COLOR,
            "--format=",
            "--diff-merges=first-parent",
            context,
            sha,
            "--",
            path,
        ]),
        DiffSource::Refs { a, b, path } => run(&["diff", EXT, COLOR, context, a, b, "--", path]),
    }
}

/// The whole file on each side of a diff, old then new. None for a side where
/// the file doesn't exist, or can't be read.
/// Both versions are read at once, as each is a git call of its own.
pub fn file_versions(source: &DiffSource) -> (Option<String>, Option<String>) {
    let path = source.path();
    let (old, new) = match source {
        // An empty revision makes `:path`, the file as staged in the index.
        DiffSource::Unstaged(_) => (Version::At(String::new()), Version::OnDisk),
        DiffSource::Staged(_) => (Version::At("HEAD".to_string()), Version::At(String::new())),
        DiffSource::Untracked(_) => (Version::Missing, Version::OnDisk),
        // Both sides are lines of the file as it is, markers and all.
        DiffSource::Conflict(_) => (Version::OnDisk, Version::OnDisk),
        // The diff is against the first parent, which `^` names.
        DiffSource::Commit { sha, .. } => (Version::At(format!("{sha}^")), Version::At(sha.clone())),
        DiffSource::Refs { a, b, .. } => (Version::At(a.clone()), Version::At(b.clone())),
    };
    thread::scope(|scope| {
        let old = scope.spawn(|| old.read(path));
        let new = new.read(path);
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
    fn read(&self, path: &str) -> Option<String> {
        match self {
            Version::At(revision) => run(&["show", &format!("{revision}:{path}")]).ok(),
            Version::OnDisk => {
                let bytes = std::fs::read(path).ok()?;
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
        });
    }
    files
}

// ------------------------------------------------------------------ graph

/// Commits reachable from `refs`, newest first. `--topo-order` guarantees that
/// children come before their parents, which the graph layout relies on.
pub fn log(refs: &[String], limit: usize) -> Result<Vec<Commit>> {
    let limit = format!("-n{limit}");
    let mut args = vec!["log", "--topo-order", limit.as_str(), "--format=%H%x00%P%x00%an%x00%s"];
    args.extend(refs.iter().map(String::as_str));
    args.push("--");
    Ok(parse_log(&run(&args)?))
}

fn parse_log(text: &str) -> Vec<Commit> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\0');
            Some(Commit {
                sha: fields.next()?.to_string(),
                parents: fields.next()?.split_whitespace().map(str::to_string).collect(),
                author: fields.next()?.to_string(),
                subject: fields.next()?.to_string(),
            })
        })
        .collect()
}

/// The commits reachable from `refs` that aren't on any remote-tracking branch.
pub fn unpushed(refs: &[String]) -> Result<HashSet<String>> {
    let mut args = vec!["rev-list"];
    args.extend(refs.iter().map(String::as_str));
    args.extend(["--not", "--remotes", "--"]);
    Ok(run(&args)?.lines().map(str::to_string).collect())
}

/// Changes whenever a ref or HEAD moves, for spotting commits, checkouts and
/// fetches made outside the app without reloading the whole graph. HEAD names
/// the checked-out branch, whose commit is among the refs, or a commit itself.
pub fn refs_fingerprint() -> String {
    let refs = try_run(&["for-each-ref", "--format=%(objectname) %(refname)"]);
    format!("{}\n{}", refs.unwrap_or_default(), read_head().unwrap_or_default())
}

/// The repo's git directory, asked for once. Some questions are answered by
/// reading its files, which is much quicker than starting git.
fn git_dir() -> Option<&'static Path> {
    static GIT_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    GIT_DIR
        .get_or_init(|| try_run(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from))
        .as_deref()
}

/// What HEAD holds: `ref: refs/heads/main`, or a commit's sha when detached.
fn read_head() -> Option<String> {
    let head = std::fs::read_to_string(git_dir()?.join("HEAD")).ok()?;
    Some(head.trim().to_string())
}

/// A merge, rebase, cherry-pick or revert that stopped halfway, usually on
/// conflicts, as the files git leaves in its directory while one is going.
pub fn operation_in_progress() -> Option<&'static str> {
    let dir = git_dir()?;
    let operations = [
        ("MERGE_HEAD", "merging"),
        ("rebase-merge", "rebasing"),
        ("rebase-apply", "rebasing"),
        ("CHERRY_PICK_HEAD", "cherry-picking"),
        ("REVERT_HEAD", "reverting"),
    ];
    operations.iter().find(|(file, _)| dir.join(file).exists()).map(|(_, name)| *name)
}

/// Where a branch is checked out, if that's another worktree than this one:
/// `../name` when it's next to this one, as worktrees usually are, or else the
/// whole path. None for this worktree.
pub fn other_worktree(path: &str) -> Option<String> {
    let here = std::env::current_dir().ok()?.canonicalize().ok()?;
    let there = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    if there == here {
        return None;
    }
    match (there.parent(), here.parent(), there.file_name()) {
        (Some(a), Some(b), Some(name)) if a == b => Some(format!("../{}", name.to_string_lossy())),
        _ => Some(path.to_string()),
    }
}

/// The checked-out branch. None on a detached HEAD.
pub fn current_branch() -> Option<String> {
    read_head()?.strip_prefix("ref: refs/heads/").map(str::to_string)
}

/// The default graph view: the current branch and its remote counterpart,
/// which is its upstream or else origin's branch of the same name. Refs that
/// don't exist, like that one or a branch with no commits yet, are dropped
/// later, when `resolve_all` finds nothing for them.
pub fn default_refs() -> Vec<String> {
    let Some(branch) = current_branch() else {
        return vec!["HEAD".to_string()]; // detached HEAD
    };
    let upstream = try_run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
    let remote = upstream.unwrap_or_else(|| format!("origin/{branch}"));
    vec![branch, remote]
}

/// The commit each ref points at, in the same order, with None for those that
/// don't exist. One git call does them all, unless one of them doesn't exist,
/// which fails it: then they're asked for one by one.
pub fn resolve_all(refs: &[String]) -> Vec<Option<String>> {
    // A ref starting with a dash would be read as a flag.
    if !refs.is_empty() && refs.iter().all(|reference| !reference.starts_with('-')) {
        let commits: Vec<String> = refs.iter().map(|r| format!("{r}^{{commit}}")).collect();
        let mut args = vec!["rev-parse"];
        args.extend(commits.iter().map(String::as_str));
        if let Ok(text) = run(&args) {
            let shas: Vec<Option<String>> = text.lines().map(|sha| Some(sha.to_string())).collect();
            if shas.len() == refs.len() {
                return shas;
            }
        }
    }
    refs.iter().map(|reference| resolve(reference)).collect()
}

/// The commit sha a ref points at, if it exists.
pub fn resolve(reference: &str) -> Option<String> {
    if reference.starts_with('-') {
        return None; // would be read as a flag
    }
    let commit = format!("{reference}^{{commit}}");
    try_run(&["rev-parse", "--verify", "-q", commit.as_str()])
}

/// Whether this could be a commit sha, full or abbreviated as git usually does.
pub fn looks_like_sha(text: &str) -> bool {
    (7..=40).contains(&text.len()) && text.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// A ref as worth showing on screen: shas get shortened, names stay as they are.
pub fn display_ref(reference: &str) -> String {
    if looks_like_sha(reference) {
        let commit = format!("{reference}^{{commit}}");
        if let Some(short) = try_run(&["rev-parse", "--short", commit.as_str()]) {
            return short;
        }
    }
    reference.to_string()
}

/// A one-line description of a commit: its short sha, two spaces, its subject.
pub fn commit_label(sha: &str) -> Option<String> {
    try_run(&["show", "-s", "--format=%h  %s", sha])
}

pub fn commit_details(sha: &str) -> Result<CommitDetails> {
    let text = run(&["show", "-s", "--date=iso", "--format=%H%n%an <%ae>%n%ad%n%B", sha])?;
    let mut lines = text.splitn(4, '\n');
    Ok(CommitDetails {
        sha: lines.next().unwrap_or("").to_string(),
        author: lines.next().unwrap_or("").to_string(),
        date: lines.next().unwrap_or("").to_string(),
        message: lines.next().unwrap_or("").trim_end().to_string(),
    })
}

pub fn commit_files(sha: &str) -> Result<Vec<FileEntry>> {
    let text = run(&[
        "show",
        "--format=",
        "--name-status",
        "--no-renames",
        "--diff-merges=first-parent",
        "-z",
        sha,
    ])?;
    Ok(parse_name_status(&text))
}

// --------------------------------------------------------------- branches

/// Local branches first, then remote ones.
pub fn branches() -> Result<Vec<Branch>> {
    let text = run(&[
        "for-each-ref",
        "--format=%(refname)%00%(refname:short)%00%(refname:lstrip=3)%00%(upstream:short)%00%(upstream:track,nobracket)%00%(worktreepath)",
        "refs/heads",
        "refs/remotes",
    ])?;
    Ok(parse_branches(&text))
}

fn parse_branches(text: &str) -> Vec<Branch> {
    let mut branches = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [full, short, without_remote, upstream, track, worktree] = fields[..] else { continue };
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
                ahead_behind: if upstream.is_empty() { None } else { parse_track(track) },
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

pub fn tags() -> Result<Vec<String>> {
    let text = run(&["for-each-ref", "--format=%(refname:short)", "refs/tags"])?;
    Ok(text.lines().map(str::to_string).collect())
}

pub fn switch(local_name: &str) -> Result<()> {
    run(&["switch", local_name]).map(|_| ())
}

/// Creates a local branch tracking this remote one, and switches to it.
pub fn switch_track(remote_name: &str) -> Result<()> {
    run(&["switch", "--track", remote_name]).map(|_| ())
}

// ---------------------------------------------------------------- compare

/// How many commits `a` has that `b` doesn't, and the other way around.
pub fn ahead_behind(a: &str, b: &str) -> Result<(usize, usize)> {
    let range = format!("{a}...{b}");
    let text = run(&["rev-list", "--left-right", "--count", range.as_str(), "--"])?;
    let mut counts = text.split_whitespace().map(|n| n.parse::<usize>().unwrap_or(0));
    Ok((counts.next().unwrap_or(0), counts.next().unwrap_or(0)))
}

pub fn merge_base(a: &str, b: &str) -> Option<String> {
    try_run(&["merge-base", a, b])
}

pub fn changed_files(a: &str, b: &str) -> Result<Vec<FileEntry>> {
    let text = run(&["diff", "--name-status", "--no-renames", "-z", a, b, "--"])?;
    Ok(parse_name_status(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(status: char, path: &str) -> FileEntry {
        FileEntry { status, path: path.to_string() }
    }

    #[test]
    fn status_splits_entries_into_staged_and_unstaged() {
        let status = parse_status("M  staged.rs\0 M unstaged.rs\0MM both.rs\0?? new file.rs\0");
        assert_eq!(status.staged, vec![entry('M', "staged.rs"), entry('M', "both.rs")]);
        assert_eq!(
            status.unstaged,
            vec![entry('M', "unstaged.rs"), entry('M', "both.rs"), entry('?', "new file.rs")]
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
        assert_eq!(status.conflicted, vec![entry('U', "a"), entry('U', "b"), entry('U', "c")]);
        assert_eq!(status.unstaged, vec![entry('M', "d")]);
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

    // These run against gclic's own repo, comparing the quick answers with
    // what git itself says.

    #[test]
    fn current_branch_agrees_with_git() {
        let from_git = try_run(&["symbolic-ref", "--short", "-q", "HEAD"]);
        assert_eq!(current_branch(), from_git);
    }

    #[test]
    fn resolving_several_refs_at_once_matches_one_at_a_time() {
        let refs = ["HEAD".to_string(), "HEAD~1".to_string()];
        let expected: Vec<_> = refs.iter().map(|r| resolve(r)).collect();
        assert!(expected.iter().all(Option::is_some));
        assert_eq!(resolve_all(&refs), expected);
    }

    #[test]
    fn a_missing_ref_resolves_to_nothing_without_losing_the_others() {
        let refs = ["HEAD".to_string(), "no-such-branch-here".to_string(), "-x".to_string()];
        let shas = resolve_all(&refs);
        assert_eq!(shas[0], resolve("HEAD"));
        assert_eq!(shas[1..], [None, None]);
    }

    #[test]
    fn a_whole_file_diff_holds_every_line_of_the_file() {
        // The first commit's Cargo.toml was edited later, so a diff against it
        // has changes, and unchanged lines that only a whole-file diff shows.
        let first = log(&["HEAD".to_string()], 10_000).unwrap().pop().unwrap();
        let source = DiffSource::Refs { a: first.sha, b: "HEAD".to_string(), path: "Cargo.toml".to_string() };
        let changes = diff_with(&source, false).unwrap();
        let whole = diff_with(&source, true).unwrap();
        let lines_now = run(&["show", "HEAD:Cargo.toml"]).unwrap().lines().count();
        let shown = |diff: &str| {
            let new_side = |l: &&str| l.starts_with(' ') || (l.starts_with('+') && !l.starts_with("+++"));
            diff.lines().filter(new_side).count()
        };
        assert_eq!(shown(&whole), lines_now);
        assert!(shown(&changes) <= shown(&whole));
    }

    #[test]
    fn both_versions_of_a_file_come_back() {
        let source = DiffSource::Commit { sha: "HEAD".to_string(), path: "Cargo.toml".to_string() };
        let (old, new) = file_versions(&source);
        assert!(old.is_some_and(|text| text.contains("[package]")));
        assert!(new.is_some_and(|text| text.contains("[package]")));
    }
}
