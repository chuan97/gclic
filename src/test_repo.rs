//! Repos made for a test, so each one sets up the situation it tests, a stash
//! or a conflict, say, without depending on gclic's own history, and without
//! getting in the way of the tests running beside it. Each is a folder of
//! its own under the system's temporary folder, removed when it's dropped.

use crate::git::{self, Repo};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct TestRepo {
    pub repo: Repo,
    folder: PathBuf,
}

impl std::ops::Deref for TestRepo {
    type Target = Repo;

    fn deref(&self) -> &Repo {
        &self.repo
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// A folder no other test, or earlier run, uses.
fn fresh_folder() -> PathBuf {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let count = COUNT.fetch_add(1, Ordering::Relaxed);
    let folder = std::env::temp_dir().join(format!("gclic-test-{}-{count}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    // As git names it, so paths compare equal: on macOS the temporary folder
    // is a link into /private.
    folder.canonicalize().unwrap()
}

/// Git in `folder`, with the user's and the system's settings left out, as
/// they are from gclic's own git calls in tests.
fn git_command(folder: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.args(args).current_dir(folder);
    git::without_user_config(&mut command);
    command
}

/// Runs git in `folder`, failing the test if git fails, and returns what it
/// printed.
fn git_in(folder: &Path, args: &[&str]) -> String {
    let output = git_command(folder, args).output().unwrap();
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    assert!(output.status.success(), "git {} failed: {}{}", args.join(" "), text(&output.stderr), text(&output.stdout));
    text(&output.stdout)
}

/// What a repo is set up with, in its own config.
const CONFIG: &[(&str, &str)] = &[
    ("user.name", "Test"),
    ("user.email", "test@example.com"),
    ("commit.gpgsign", "false"),
    ("tag.gpgsign", "false"),
    ("core.autocrlf", "false"),
    ("core.hooksPath", "/dev/null"),
    ("merge.conflictStyle", "merge"),
    ("advice.detachedHead", "false"),
];

impl TestRepo {
    /// An empty repo on `main`, without a commit.
    pub fn new() -> TestRepo {
        let folder = fresh_folder();
        git_in(&folder, &["init", "-q", "-b", "main"]);
        for (key, value) in CONFIG {
            git_in(&folder, &["config", key, value]);
        }
        let repo = Repo::discover(&folder).unwrap();
        TestRepo { repo, folder }
    }

    /// A repo with a short history on `main`: `src/main.rs` and a README,
    /// then each edited, plus a branch, `feature`, with a commit of its own.
    /// `main` is checked out.
    pub fn with_history() -> TestRepo {
        let repo = TestRepo::new();
        repo.write("src/main.rs", "fn main() {\n    println!(\"one\");\n}\n");
        repo.write("README.md", "# Test\n");
        repo.commit("First");
        repo.write("src/main.rs", "fn main() {\n    let n = 2;\n    println!(\"{n}\");\n}\n");
        repo.commit("Second");
        repo.write("README.md", "# Test\n\nMore.\n");
        repo.commit("Third");
        repo.git(&["switch", "-q", "-c", "feature"]);
        repo.write("feature.txt", "new\n");
        repo.commit("On feature");
        repo.git(&["switch", "-q", "main"]);
        repo
    }

    /// Another repo, a clone of this one, with this one as its `origin`.
    pub fn clone_of(&self) -> TestRepo {
        let folder = fresh_folder();
        git_in(&folder, &["clone", "-q", &self.root().display().to_string(), "."]);
        for (key, value) in CONFIG {
            git_in(&folder, &["config", key, value]);
        }
        let repo = Repo::discover(&folder).unwrap();
        TestRepo { repo, folder }
    }

    /// Runs git here, failing the test if git fails, and returns what it
    /// printed.
    pub fn git(&self, args: &[&str]) -> String {
        git_in(self.root(), args)
    }

    /// Like `git`, but for commands expected to fail, as a merge that stops on
    /// conflicts does. Whether it succeeded.
    pub fn git_may_fail(&self, args: &[&str]) -> bool {
        git_command(self.root(), args).output().unwrap().status.success()
    }

    /// Writes a file, by its path from the root, making its folders.
    pub fn write(&self, path: &str, text: &str) {
        let file = self.file(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    pub fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.file(path)).ok()
    }

    /// Commits everything, untracked files included, and returns the sha.
    pub fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", message]);
        self.sha("HEAD")
    }

    /// The commit a ref points at.
    pub fn sha(&self, reference: &str) -> String {
        self.git(&["rev-parse", &format!("{reference}^{{commit}}")]).trim().to_string()
    }

    /// Changes `path` differently on `main` and on a new branch, `other`, then
    /// merges `other` into `main`, which stops on the conflict.
    pub fn conflicting_merge(&self, path: &str) {
        self.write(path, "base\n");
        self.commit("Base");
        self.git(&["switch", "-q", "-c", "other"]);
        self.write(path, "theirs\n");
        self.commit("Theirs");
        self.git(&["switch", "-q", "main"]);
        self.write(path, "ours\n");
        self.commit("Ours");
        assert!(!self.git_may_fail(&["merge", "other"]), "the merge should stop on the conflict");
    }
}
