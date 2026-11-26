//! Undoing what gclic stages, unstages and discards, and redoing it, a step
//! at a time, for as long as gclic runs in the repo. Each step keeps what it touched as it
//! was before and after: the files' entries in the index, or the files
//! themselves, kept in git's object store. Undoing puts back the before,
//! but only while the after is still there, so nothing done since, in gclic
//! or anywhere else, is lost to an undo. Redoing is the same the other way.

use crate::git::{self, Repo};
use anyhow::{Result, anyhow};

/// The most steps kept. Older ones go.
const MOST_STEPS: usize = 100;

/// What a step touched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// What's staged, as staging and unstaging change it.
    Index,
    /// The files themselves, as discarding changes them.
    WorkingTree,
}

#[derive(Debug, Clone, PartialEq)]
enum State {
    Index(String),
    Files(Vec<Option<String>>),
}

impl State {
    fn read(repo: &Repo, kind: Kind, paths: &[String]) -> Result<State> {
        Ok(match kind {
            Kind::Index => State::Index(git::index_entries(repo, paths)?),
            Kind::WorkingTree => {
                State::Files(paths.iter().map(|path| git::save_file(repo, path)).collect::<Result<_>>()?)
            }
        })
    }

    fn put_back(&self, repo: &Repo, paths: &[String]) -> Result<()> {
        match self {
            State::Index(entries) => git::set_index_entries(repo, paths, entries),
            State::Files(files) => {
                for (path, saved) in paths.iter().zip(files) {
                    git::restore_file(repo, path, saved.as_deref())?;
                }
                Ok(())
            }
        }
    }
}

struct Step {
    /// What it did, as "staging a change to f", for the message saying it
    /// was undone.
    label: String,
    kind: Kind,
    paths: Vec<String>,
    before: State,
    after: State,
}

#[derive(Default)]
pub struct History {
    steps: Vec<Step>,
    /// The steps undone, the latest last, until a new step is made.
    undone: Vec<Step>,
}

impl History {
    pub fn can_undo(&self) -> bool {
        !self.steps.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    pub fn clear(&mut self) {
        self.steps.clear();
        self.undone.clear();
    }

    /// Does `operation`, which changes `paths` in the way `kind` says, and
    /// keeps it as a step to undo, named `label`. If what it touches can't be
    /// read, it's done all the same, just not kept.
    pub fn record(
        &mut self,
        repo: &Repo,
        label: String,
        kind: Kind,
        paths: Vec<String>,
        operation: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        if paths.is_empty() {
            return operation();
        }
        let before = State::read(repo, kind, &paths);
        operation()?;
        if let (Ok(before), Ok(after)) = (before, State::read(repo, kind, &paths))
            && before != after
        {
            self.steps.push(Step { label, kind, paths, before, after });
            // A new step starts another line of history, as in vim.
            self.undone.clear();
            if self.steps.len() > MOST_STEPS {
                self.steps.remove(0);
            }
        }
        Ok(())
    }

    /// Undoes the latest step, returning what it did. A step whose files
    /// have changed since is dropped rather than undone, as undoing it would
    /// lose those changes, and the error says so: the next undo goes on to
    /// the one before. None if there's nothing to undo.
    pub fn undo(&mut self, repo: &Repo) -> Option<Result<String>> {
        let step = self.steps.pop()?;
        let result = step.go(repo, "undo", &step.after, &step.before);
        if result.is_ok() {
            self.undone.push(step);
        }
        Some(result)
    }

    /// Redoes the latest step undone, the same way.
    pub fn redo(&mut self, repo: &Repo) -> Option<Result<String>> {
        let step = self.undone.pop()?;
        let result = step.go(repo, "redo", &step.before, &step.after);
        if result.is_ok() {
            self.steps.push(step);
        }
        Some(result)
    }
}

impl Step {
    /// Puts back `to`, if the files are still as `from` has them.
    fn go(&self, repo: &Repo, verb: &str, from: &State, to: &State) -> Result<String> {
        if State::read(repo, self.kind, &self.paths)? != *from {
            let what = match self.paths.as_slice() {
                [path] => path.clone(),
                _ => "the files".to_string(),
            };
            return Err(anyhow!("Can't {verb} {}: {what} changed since", self.label));
        }
        to.put_back(repo, &self.paths)?;
        Ok(self.label.clone())
    }
}
