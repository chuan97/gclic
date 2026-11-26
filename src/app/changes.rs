//! Staging, unstaging and discarding, a file or a change at a time, and
//! undoing them with `u`.

use super::*;
use crate::git::FileEntry;

impl App {
    /// Inside a diff of the working tree, where space and d take a change.
    pub(super) fn in_working_tree_diff(&self) -> bool {
        self.view.in_preview && self.view.diff().is_some_and(|diff| diff.source.is_working_tree())
    }

    /// Where files and changes are staged and discarded, and so where u and
    /// ctrl-r undo and redo that: the Changes pane, and inside a diff of the
    /// working tree.
    pub(super) fn where_staging_is(&self) -> bool {
        self.view.focused(Pane::Changes) || self.in_working_tree_diff()
    }

    /// Does a staging or a discard, keeping it to undo, then refreshes, or
    /// shows why it failed. Whether it worked.
    fn record(
        &mut self,
        label: String,
        kind: undo::Kind,
        paths: Vec<String>,
        operation: impl FnOnce(&Repo) -> Result<()>,
    ) -> bool {
        let repo = self.repo.clone();
        let result = self.history.record(&repo, label, kind, paths, || operation(&repo));
        let worked = result.is_ok();
        self.apply(Action::after_change(result));
        worked
    }

    /// The Changes pane's space, a and A.
    pub(super) fn stage(&mut self, staging: Staging) {
        // Only staging or unstaging everything needs to know what that is.
        let status = || git::status(&self.repo).unwrap_or_default();
        let paths_of = |lists: &[&Vec<FileEntry>]| {
            let mut paths: Vec<String> = Vec::new();
            for entry in lists.iter().flat_map(|list| list.iter()) {
                if !paths.contains(&entry.path) {
                    paths.push(entry.path.clone());
                }
            }
            paths
        };
        let (label, paths) = match &staging {
            Staging::Stage(path) => (format!("staging {path}"), vec![path.clone()]),
            Staging::Unstage(path) => (format!("unstaging {path}"), vec![path.clone()]),
            Staging::StageAll => {
                let status = status();
                ("staging everything".to_string(), paths_of(&[&status.conflicted, &status.unstaged]))
            }
            Staging::UnstageAll => ("unstaging everything".to_string(), paths_of(&[&status().staged])),
        };
        self.record(label, undo::Kind::Index, paths, |repo| match &staging {
            Staging::Stage(path) => git::stage(repo, path),
            Staging::Unstage(path) => git::unstage(repo, path),
            Staging::StageAll => git::stage_all(repo),
            Staging::UnstageAll => git::unstage_all(repo),
        });
    }

    /// Space inside a working-tree diff: stages the change the active line is
    /// in, from the unstaged diff, or unstages it, from the staged one, then
    /// goes on to the next change. A file whose diff is a single change goes
    /// whole, as `space` in the list would take it, which also stages a new
    /// file or a deletion, which a patch of its lines couldn't.
    pub(super) fn stage_active_change(&mut self) {
        let Some(pane) = self.view.diff() else {
            return;
        };
        let Some(change) = pane.active_change() else {
            self.message = "Not on a change: n goes to the next one".to_string();
            return;
        };
        let (source, text, whole) = (pane.source.clone(), pane.text().to_string(), pane.change_count() == 1);
        let path = source.path().to_string();
        let staging = match &source {
            DiffSource::Unstaged(_) | DiffSource::Untracked(_) => true,
            DiffSource::Staged(_) => false,
            DiffSource::Conflict(_) => {
                self.message = "A conflict is staged whole once resolved, with space in the list".to_string();
                return;
            }
            _ => return,
        };
        // A new file is all one change.
        let whole = whole || matches!(source, DiffSource::Untracked(_));
        let done = if staging { "Staged" } else { "Unstaged" };
        let label = match whole {
            true => format!("{} {path}", if staging { "staging" } else { "unstaging" }),
            false => format!("{} a change to {path}", if staging { "staging" } else { "unstaging" }),
        };
        let worked = self.record(label, undo::Kind::Index, vec![path.clone()], |repo| match (whole, staging) {
            (true, true) => git::stage(repo, &path),
            (true, false) => git::unstage(repo, &path),
            (false, _) => {
                let direction = if staging { diff::Direction::Forward } else { diff::Direction::Reverse };
                match diff::change_patch(&text, &path, &change, direction) {
                    Some(patch) => git::apply_to_index(repo, &patch, !staging),
                    None => Err(anyhow::anyhow!("That change isn't in the diff any more")),
                }
            }
        });
        if worked {
            self.message = if whole { format!("{done} {path}") } else { format!("{done} the change") };
            if let Some(pane) = self.view.diff_mut() {
                pane.go_to_nearest_change();
            }
        }
    }

    /// d inside a working-tree diff: asks to discard the change the active
    /// line is in, from the unstaged diff. A file whose diff is a single
    /// change is discarded whole, as `d` in the list would, and an untracked
    /// one deleted, after asking the same.
    pub(super) fn ask_to_restore_active_change(&mut self) {
        let Some(pane) = self.view.diff() else {
            return;
        };
        let Some(change) = pane.active_change() else {
            self.message = "Not on a change: n goes to the next one".to_string();
            return;
        };
        let path = pane.source.path().to_string();
        self.modal = match &pane.source {
            DiffSource::Untracked(_) => Modal::Confirm(Confirm::DeleteUntracked(path)),
            DiffSource::Unstaged(_) if pane.change_count() == 1 => Modal::Confirm(Confirm::Restore(path)),
            DiffSource::Unstaged(_) => match diff::change_patch(pane.text(), &path, &change, diff::Direction::Reverse) {
                Some(patch) => Modal::Confirm(Confirm::RestoreChange { path, patch }),
                None => Modal::Error("That change isn't in the diff any more".to_string()),
            },
            DiffSource::Staged(_) => {
                self.message = "Staged changes are kept: unstage it first, with space".to_string();
                return;
            }
            _ => {
                self.message = "A conflict is resolved in the file, then staged whole".to_string();
                return;
            }
        };
    }

    /// A file's changes thrown away, once confirmed, kept to undo.
    pub(super) fn restore_file(&mut self, path: &str) {
        let label = format!("discarding your changes to {path}");
        self.record(label, undo::Kind::WorkingTree, vec![path.to_string()], |repo| git::restore(repo, path));
    }

    /// One change of a file thrown away, once confirmed, kept to undo.
    pub(super) fn restore_change(&mut self, path: &str, patch: &str) {
        let label = format!("discarding a change to {path}");
        let paths = vec![path.to_string()];
        if self.record(label, undo::Kind::WorkingTree, paths, |repo| git::unapply_from_working_tree(repo, patch)) {
            self.message = "Discarded the change".to_string();
            if let Some(pane) = self.view.diff_mut() {
                pane.go_to_nearest_change();
            }
        }
    }

    /// D, once confirmed. The changes are kept to undo, but not the untracked
    /// files deleted, which git never had.
    pub(super) fn discard_all(&mut self, restore: &[String], delete: &[String]) {
        let files = |count: usize, kind: &str| if count == 1 { format!("1 {kind}file") } else { format!("{count} {kind}files") };
        let mut label = format!("discarding your changes to {}", files(restore.len(), ""));
        if !delete.is_empty() {
            label.push_str(&format!(", though not deleting {}", files(delete.len(), "untracked ")));
        }
        self.record(label, undo::Kind::WorkingTree, restore.to_vec(), |repo| git::discard_all(repo, restore, delete));
    }

    /// u: undoes the latest staging or discard made in gclic, and ctrl-r
    /// redoes the latest undone.
    pub(super) fn undo(&mut self, redo: bool) {
        let result = if redo { self.history.redo(&self.repo) } else { self.history.undo(&self.repo) };
        let Some(result) = result else {
            self.message = if redo { "Nothing to redo" } else { "Nothing to undo" }.to_string();
            return;
        };
        self.refresh();
        self.message = match result {
            Ok(label) if redo => format!("Redid {label}"),
            Ok(label) => format!("Undid {label}"),
            Err(error) => error.to_string(),
        };
    }
}
