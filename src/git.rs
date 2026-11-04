//! Every interaction with git lives here. The rest of the app never sees a
//! subprocess or raw git output, only the typed data returned by this module.

use anyhow::{Result, anyhow, bail};
use std::collections::HashSet;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    /// Status letter: M, A, D, ... plus `?` for untracked and `U` for conflicts.
    pub status: char,
    pub path: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Status {
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
}

/// What a diff pane is showing. One variant per place a diff can come from.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffSource {
    Unstaged(String),
    Staged(String),
    Untracked(String),
    Commit { sha: String, path: String },
    Refs { a: String, b: String, path: String },
}

impl DiffSource {
    /// Whether this comes from the Changes pane, rather than from history.
    pub fn is_working_tree(&self) -> bool {
        matches!(self, DiffSource::Unstaged(_) | DiffSource::Staged(_) | DiffSource::Untracked(_))
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
            status.unstaged.push(FileEntry { status: 'U', path });
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

pub fn diff(source: &DiffSource) -> Result<String> {
    const EXT: &str = "--no-ext-diff";
    const COLOR: &str = "--no-color";
    match source {
        DiffSource::Unstaged(path) => run(&["diff", EXT, COLOR, "--", path]),
        DiffSource::Staged(path) => run(&["diff", EXT, COLOR, "--cached", "--", path]),
        DiffSource::Untracked(path) => {
            // --no-index exits with 1 when the files differ, which here is
            // always, so the exit code can't be treated as failure.
            let output = git(&["diff", EXT, COLOR, "--no-index", "--", "/dev/null", path])?;
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        DiffSource::Commit { sha, path } => run(&[
            "show",
            EXT,
            COLOR,
            "--format=",
            "--diff-merges=first-parent",
            sha,
            "--",
            path,
        ]),
        DiffSource::Refs { a, b, path } => run(&["diff", EXT, COLOR, a, b, "--", path]),
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

fn has_commits() -> bool {
    try_run(&["rev-parse", "--verify", "-q", "HEAD"]).is_some()
}

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
/// fetches made outside the app without reloading the whole graph.
pub fn refs_fingerprint() -> String {
    let refs = try_run(&["for-each-ref", "--format=%(objectname) %(refname)"]);
    let head = try_run(&["rev-parse", "HEAD", "--symbolic-full-name", "HEAD"]);
    format!("{}\n{}", refs.unwrap_or_default(), head.unwrap_or_default())
}

/// The checked-out branch. None on a detached HEAD.
pub fn current_branch() -> Option<String> {
    try_run(&["symbolic-ref", "--short", "-q", "HEAD"])
}

/// The default graph view: the current branch and its remote counterpart.
pub fn default_refs() -> Vec<String> {
    if !has_commits() {
        return Vec::new();
    }
    let Some(branch) = current_branch() else {
        return vec!["HEAD".to_string()]; // detached HEAD
    };
    let upstream = try_run(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
    let origin = format!("origin/{branch}");
    let remote = upstream.or_else(|| resolve(&origin).map(|_| origin));
    let mut refs = vec![branch];
    refs.extend(remote);
    refs
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
        "--format=%(refname)%00%(refname:short)%00%(refname:lstrip=3)%00%(upstream:short)",
        "refs/heads",
        "refs/remotes",
    ])?;
    Ok(parse_branches(&text))
}

fn parse_branches(text: &str) -> Vec<Branch> {
    let mut branches = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [full, short, without_remote, upstream] = fields[..] else { continue };
        if full.starts_with("refs/remotes/") {
            // `origin/HEAD` is only a pointer to the remote's default branch.
            if without_remote != "HEAD" {
                branches.push(Branch {
                    name: short.to_string(),
                    local_name: without_remote.to_string(),
                    is_remote: true,
                    upstream: None,
                });
            }
        } else {
            branches.push(Branch {
                name: short.to_string(),
                local_name: short.to_string(),
                is_remote: false,
                upstream: (!upstream.is_empty()).then(|| upstream.to_string()),
            });
        }
    }
    branches
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
    fn status_treats_merge_conflicts_as_unstaged() {
        let status = parse_status("UU a\0AA b\0DD c\0");
        assert!(status.staged.is_empty());
        assert_eq!(status.unstaged, vec![entry('U', "a"), entry('U', "b"), entry('U', "c")]);
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
    fn branches_skip_remote_head_and_strip_the_remote() {
        let text = "refs/heads/main\0main\0\0origin/main\n\
                    refs/remotes/origin/HEAD\0origin\0HEAD\0\n\
                    refs/remotes/origin/a/b\0origin/a/b\0a/b\0\n";
        let branches = parse_branches(text);
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].local_name, "main");
        assert_eq!(branches[1].name, "origin/a/b");
        assert_eq!(branches[1].local_name, "a/b");
        assert_eq!(branches[0].upstream.as_deref(), Some("origin/main"));
        assert!(!branches[0].is_remote);
        assert!(branches[1].is_remote);
    }
}
