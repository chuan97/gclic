//! `git.rs` against real repos, each made for its test, set up as the user
//! would leave one: changes staged and not, conflicts halfway through a
//! merge or a rebase, stashes, remotes and worktrees.

use super::*;
use crate::test_repo::TestRepo;

fn entry(status: char, path: &str) -> FileEntry {
    FileEntry {
        status,
        path: path.to_string(),
        ..Default::default()
    }
}

fn paths(files: &[FileEntry]) -> Vec<&str> {
    files.iter().map(|file| file.path.as_str()).collect()
}

/// Stashes by their shas, as their ages change while a test runs.
fn shas(stashes: &[Stash]) -> Vec<&str> {
    stashes.iter().map(|stash| stash.sha.as_str()).collect()
}

fn names(branches: &[Branch]) -> Vec<&str> {
    branches.iter().map(|branch| branch.name.as_str()).collect()
}

fn unstaged(path: &str) -> DiffSource {
    DiffSource::Unstaged(path.to_string())
}

// ------------------------------------------------------------- the repo

#[test]
fn two_repos_answer_for_themselves_whatever_the_working_directory() {
    // The tests run in gclic's folder, neither of these.
    let (a, b) = (TestRepo::with_history(), TestRepo::new());
    a.write("in-a.txt", "a\n");
    b.write("in-b.txt", "b\n");
    assert_eq!(paths(&status(&a).unwrap().unstaged), ["in-a.txt"]);
    assert_eq!(paths(&status(&b).unwrap().unstaged), ["in-b.txt"]);
    assert_eq!(current_branch(&a).as_deref(), Some("main"));
    assert!(
        log(&b, &["HEAD".to_string()], 10).is_err(),
        "b has no commits"
    );
}

#[test]
fn an_empty_repo_reads_as_empty_rather_than_failing() {
    let repo = TestRepo::new();
    assert_eq!(status(&repo).unwrap(), Status::default());
    // HEAD names a branch that has no commit yet.
    assert_eq!(current_branch(&repo).as_deref(), Some("main"));
    assert_eq!(default_refs(&repo), ["main", "origin/main"]);
    assert_eq!(resolve_all(&repo, &default_refs(&repo)), [None, None]);
    assert!(stashes(&repo).unwrap().is_empty());
    assert!(branches(&repo).unwrap().is_empty());
    assert!(tags(&repo).unwrap().is_empty());
    assert_eq!(operation_in_progress(&repo), None);
}

#[test]
fn settings_are_read_and_written_in_the_repo_s_own_config() {
    let repo = TestRepo::new();
    assert_eq!(local_config(&repo, "gclic.test"), None);
    set_local_config(&repo, "gclic.test", Some("on")).unwrap();
    assert_eq!(local_config(&repo, "gclic.test").as_deref(), Some("on"));
    // Booleans as git reads them, in any of its spellings.
    set_local_config(&repo, "gclic.flag", Some("off")).unwrap();
    assert_eq!(config_bool(&repo, "gclic.flag"), Some(false));
    assert_eq!(config_bool(&repo, "gclic.test"), Some(true));
    assert_eq!(config_bool(&repo, "gclic.missing"), None);
    set_local_config(&repo, "gclic.test", None).unwrap();
    assert_eq!(local_config(&repo, "gclic.test"), None);
    // Removing one that isn't there is fine.
    set_local_config(&repo, "gclic.test", None).unwrap();
    // The other repos don't see it.
    assert_eq!(local_config(&TestRepo::new(), "gclic.flag"), None);
}

#[test]
fn a_command_the_user_types_runs_in_the_repo() {
    let repo = TestRepo::with_history();
    let output = user_command(
        &repo,
        &["rev-parse".to_string(), "--show-toplevel".to_string()],
    )
    .output()
    .unwrap();
    assert!(output.status.success());
    assert_eq!(
        Path::new(String::from_utf8_lossy(&output.stdout).trim()),
        repo.root()
    );
}

// ---------------------------------------------------------------- staging

#[test]
fn status_reads_each_kind_of_change() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "staged\n");
    repo.git(&["add", "README.md"]);
    repo.write("src/main.rs", "fn main() {}\n");
    repo.write("new folder/new file.txt", "new\n");
    let status = status(&repo).unwrap();
    assert_eq!(status.staged, [entry('M', "README.md")]);
    // Untracked files are listed one by one, even inside a new folder.
    assert_eq!(
        status.unstaged,
        [
            entry('M', "src/main.rs"),
            entry('?', "new folder/new file.txt")
        ]
    );
    assert!(status.conflicted.is_empty());
}

#[test]
fn a_file_changed_after_staging_is_in_both_lists() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "staged\n");
    stage(&repo, "README.md").unwrap();
    repo.write("README.md", "then changed again\n");
    let status = status(&repo).unwrap();
    assert_eq!(
        (status.staged, status.unstaged),
        (vec![entry('M', "README.md")], vec![entry('M', "README.md")])
    );
    // Each diff is its own half of the change.
    let staged = diff(&repo, &DiffSource::Staged("README.md".to_string()), false).unwrap();
    let unstaged = diff(&repo, &unstaged("README.md"), false).unwrap();
    assert!(staged.contains("+staged") && staged.contains("-More."));
    assert!(unstaged.contains("-staged") && unstaged.contains("+then changed again"));
}

#[test]
fn staging_and_unstaging_move_files_between_the_lists() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "changed\n");
    repo.write("new.txt", "new\n");
    stage(&repo, "new.txt").unwrap();
    assert_eq!(status(&repo).unwrap().staged, [entry('A', "new.txt")]);
    unstage(&repo, "new.txt").unwrap();
    assert_eq!(
        status(&repo).unwrap().unstaged,
        [entry('M', "README.md"), entry('?', "new.txt")]
    );
    stage_all(&repo).unwrap();
    assert_eq!(
        status(&repo).unwrap().staged,
        [entry('M', "README.md"), entry('A', "new.txt")]
    );
    unstage_all(&repo).unwrap();
    assert!(status(&repo).unwrap().staged.is_empty());
}

#[test]
fn a_deleted_file_is_staged_as_deleted() {
    let repo = TestRepo::with_history();
    std::fs::remove_file(repo.file("README.md")).unwrap();
    assert_eq!(status(&repo).unwrap().unstaged, [entry('D', "README.md")]);
    stage(&repo, "README.md").unwrap();
    assert_eq!(status(&repo).unwrap().staged, [entry('D', "README.md")]);
}

#[test]
fn unstaging_works_before_the_first_commit() {
    let repo = TestRepo::new();
    repo.write("first.txt", "first\n");
    stage(&repo, "first.txt").unwrap();
    assert_eq!(status(&repo).unwrap().staged, [entry('A', "first.txt")]);
    unstage(&repo, "first.txt").unwrap();
    assert_eq!(status(&repo).unwrap().unstaged, [entry('?', "first.txt")]);
    stage_all(&repo).unwrap();
    unstage_all(&repo).unwrap();
    assert_eq!(status(&repo).unwrap().unstaged, [entry('?', "first.txt")]);
}

#[test]
fn restoring_and_deleting_throw_changes_away() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "changed\n");
    repo.write("new.txt", "new\n");
    restore(&repo, "README.md").unwrap();
    assert_eq!(repo.read("README.md").as_deref(), Some("# Test\n\nMore.\n"));
    delete_untracked(&repo, "new.txt").unwrap();
    assert_eq!(repo.read("new.txt"), None);
    assert_eq!(status(&repo).unwrap(), Status::default());
}

#[test]
fn discarding_everything_restores_and_deletes_at_once() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "changed\n");
    repo.write("src/main.rs", "changed\n");
    repo.write("a new.txt", "new\n");
    let restored = ["README.md".to_string(), "src/main.rs".to_string()];
    discard_all(&repo, &restored, &["a new.txt".to_string()]).unwrap();
    assert_eq!(status(&repo).unwrap(), Status::default());
    // Nothing to do is fine too.
    discard_all(&repo, &[], &[]).unwrap();
}

#[test]
fn a_path_is_only_ever_its_own_file_never_a_pattern() {
    // `[ab]` as a pattern matches `a` and `b`, and `*` everything. Windows
    // allows the brackets in a name, but not the star.
    let repo = TestRepo::with_history();
    for name in ["a", "b", "[ab]", "keep.txt"] {
        repo.write(name, "untracked\n");
    }
    stage(&repo, "[ab]").unwrap();
    assert_eq!(status(&repo).unwrap().staged, [entry('A', "[ab]")]);
    unstage(&repo, "[ab]").unwrap();
    delete_untracked(&repo, "[ab]").unwrap();
    assert_eq!(repo.read("[ab]"), None);
    for name in ["a", "b", "keep.txt"] {
        assert!(repo.read(name).is_some(), "{name} was deleted too");
    }
    repo.write("[ab]", "again\n");
    discard_all(&repo, &[], &["[ab]".to_string()]).unwrap();
    assert_eq!(
        paths(&status(&repo).unwrap().unstaged),
        ["a", "b", "keep.txt"]
    );
}

#[test]
fn committing_records_the_staged_files_and_says_when_there_are_none() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "changed\n");
    repo.write("left out.txt", "not staged\n");
    stage(&repo, "README.md").unwrap();
    commit(&repo, "Change the README\n\nWith a body.").unwrap();
    let details = commit_details(&repo, "HEAD").unwrap();
    assert_eq!(details.message, "Change the README\n\nWith a body.");
    assert_eq!(details.author, "Test <test@example.com>");
    assert_eq!(paths(&commit_files(&repo, "HEAD").unwrap()), ["README.md"]);
    // Git says why on stdout, which the error carries.
    let error = commit(&repo, "Again").unwrap_err().to_string();
    assert!(error.contains("nothing added to commit"), "{error}");
}

// ------------------------------------------------------------------ diffs

#[test]
fn each_working_tree_diff_comes_from_its_own_place() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "# Test\n\nEdited.\n");
    repo.write("docs/new.md", "one\ntwo\n");
    let changed = diff(&repo, &unstaged("README.md"), false).unwrap();
    assert!(changed.contains("-More.\n+Edited.\n"), "{changed}");
    // An untracked file, as added to nothing.
    let new = diff(
        &repo,
        &DiffSource::Untracked("docs/new.md".to_string()),
        false,
    )
    .unwrap();
    assert!(new.contains("+one\n+two\n"), "{new}");
    // Nothing staged, nothing to show.
    assert_eq!(
        diff(&repo, &DiffSource::Staged("README.md".to_string()), false).unwrap(),
        ""
    );
}

#[test]
fn each_side_of_a_working_tree_diff_is_read_from_its_own_place() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "in the index\n");
    stage(&repo, "README.md").unwrap();
    repo.write("README.md", "on disk\n");
    repo.write("new.txt", "new\n");
    let versions = |source: DiffSource| file_versions(&repo, &source);
    let some = |text: &str| Some(text.to_string());
    // Staged: from HEAD to the index. Unstaged: from the index to the disk.
    assert_eq!(
        versions(DiffSource::Staged("README.md".to_string())),
        (some("# Test\n\nMore.\n"), some("in the index\n"))
    );
    assert_eq!(
        versions(unstaged("README.md")),
        (some("in the index\n"), some("on disk\n"))
    );
    assert_eq!(
        versions(DiffSource::Untracked("new.txt".to_string())),
        (None, some("new\n"))
    );
}

#[test]
fn a_commit_s_diff_is_against_its_first_parent_even_for_a_merge() {
    let repo = TestRepo::with_history();
    repo.git(&["merge", "-q", "--no-ff", "-m", "Merge feature", "feature"]);
    let merge = repo.sha("HEAD");
    // The merge brings feature.txt, from its second parent.
    assert_eq!(
        commit_files(&repo, &merge).unwrap(),
        [FileEntry {
            status: 'A',
            path: "feature.txt".to_string(),
            lines: Lines::Counted {
                added: 1,
                removed: 0
            },
        }]
    );
    let source = DiffSource::Commit {
        sha: merge,
        path: "feature.txt".to_string(),
    };
    assert!(diff(&repo, &source, false).unwrap().contains("+new"));
    assert_eq!(
        file_versions(&repo, &source),
        (None, Some("new\n".to_string()))
    );
}

#[test]
fn a_binary_file_has_no_lines_to_count() {
    let repo = TestRepo::with_history();
    std::fs::write(repo.file("logo.png"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
    let sha = repo.commit("Add a logo");
    assert_eq!(commit_files(&repo, &sha).unwrap()[0].lines, Lines::Binary);
}

#[test]
fn comparing_two_refs_counts_their_commits_and_lists_their_files() {
    let repo = TestRepo::with_history();
    assert_eq!(ahead_behind(&repo, "main", "feature").unwrap(), (0, 1));
    assert_eq!(ahead_behind(&repo, "feature", "main").unwrap(), (1, 0));
    assert_eq!(merge_base(&repo, "main", "feature"), Some(repo.sha("main")));
    let files = changed_files(&repo, "main", "feature").unwrap();
    assert_eq!(
        files,
        [FileEntry {
            status: 'A',
            path: "feature.txt".to_string(),
            lines: Lines::Counted {
                added: 1,
                removed: 0
            }
        }]
    );
    // The other way round, the file goes.
    assert_eq!(
        changed_files(&repo, "feature", "main").unwrap()[0].status,
        'D'
    );
    let source = DiffSource::Refs {
        a: "main".to_string(),
        b: "feature".to_string(),
        path: "feature.txt".to_string(),
    };
    assert!(diff(&repo, &source, false).unwrap().contains("+new"));
}

// -------------------------------------------------------------- conflicts

#[test]
fn a_merge_conflict_is_listed_apart_and_shows_both_sides() {
    let repo = TestRepo::with_history();
    repo.conflicting_merge("clash.txt");
    let status = status(&repo).unwrap();
    assert_eq!(status.conflicted, [entry('U', "clash.txt")]);
    assert!(status.staged.is_empty() && status.unstaged.is_empty());
    assert_eq!(operation_in_progress(&repo), Some("merging"));
    // The file itself, markers and all.
    let source = DiffSource::Conflict("clash.txt".to_string());
    let text = diff(&repo, &source, false).unwrap();
    assert!(
        text.contains("<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> other\n"),
        "{text}"
    );
    let (old, new) = file_versions(&repo, &source);
    assert_eq!(old.as_deref(), Some(text.as_str()));
    assert_eq!(new.as_deref(), Some(text.as_str()));
}

#[test]
fn staging_a_conflicted_file_marks_it_resolved() {
    let repo = TestRepo::with_history();
    repo.conflicting_merge("clash.txt");
    repo.write("clash.txt", "both\n");
    stage(&repo, "clash.txt").unwrap();
    let status = status(&repo).unwrap();
    assert!(status.conflicted.is_empty());
    assert_eq!(status.staged, [entry('M', "clash.txt")]);
    // Still merging until the merge is committed.
    assert_eq!(operation_in_progress(&repo), Some("merging"));
    commit(&repo, "Merge other").unwrap();
    assert_eq!(operation_in_progress(&repo), None);
    assert_eq!(
        log(&repo, &["HEAD".to_string()], 1).unwrap()[0]
            .parents
            .len(),
        2
    );
}

#[test]
fn a_conflict_deleted_on_one_side_shows_the_file_left() {
    let repo = TestRepo::with_history();
    repo.git(&["switch", "-q", "-c", "other"]);
    repo.git(&["rm", "-q", "README.md"]);
    repo.commit("Delete the README");
    repo.git(&["switch", "-q", "main"]);
    repo.write("README.md", "edited\n");
    repo.commit("Edit the README");
    assert!(!repo.git_may_fail(&["merge", "other"]));
    assert_eq!(status(&repo).unwrap().conflicted, [entry('U', "README.md")]);
    assert_eq!(
        diff(&repo, &DiffSource::Conflict("README.md".to_string()), false).unwrap(),
        "edited\n"
    );
}

#[test]
fn each_operation_that_stops_on_a_conflict_is_told_apart() {
    let repo = TestRepo::with_history();
    repo.write("clash.txt", "base\n");
    repo.commit("Base");
    repo.git(&["switch", "-q", "-c", "other"]);
    repo.write("clash.txt", "theirs\n");
    repo.commit("Theirs");
    repo.git(&["switch", "-q", "main"]);
    repo.write("clash.txt", "ours\n");
    let ours = repo.commit("Ours");
    repo.write("clash.txt", "ours again\n");
    repo.commit("Ours again");
    assert_eq!(operation_in_progress(&repo), None);

    let stops = |args: &[&str], name: &str, abort: &str| {
        assert!(
            !repo.git_may_fail(args),
            "git {args:?} should stop on the conflict"
        );
        assert_eq!(operation_in_progress(&repo), Some(name));
        assert_eq!(status(&repo).unwrap().conflicted, [entry('U', "clash.txt")]);
        repo.git(&[abort, "--abort"]);
        assert_eq!(operation_in_progress(&repo), None);
    };
    stops(&["merge", "other"], "merging", "merge");
    stops(&["cherry-pick", "other"], "cherry-picking", "cherry-pick");
    stops(&["revert", "--no-edit", &ours], "reverting", "revert");
    repo.git(&["switch", "-q", "other"]);
    stops(&["rebase", "main"], "rebasing", "rebase");
}

// ---------------------------------------------------------------- stashes

/// A repo with one stash, of a change to the README and a new file.
fn stashed() -> TestRepo {
    let repo = TestRepo::with_history();
    repo.write("README.md", "# Test\n\nStashed.\n");
    repo.write("untracked.txt", "new\nfile\n");
    repo.git(&[
        "stash",
        "push",
        "-q",
        "--include-untracked",
        "-m",
        "try this",
    ]);
    repo
}

#[test]
fn a_stash_lists_its_base_and_its_files_tracked_and_untracked() {
    let repo = stashed();
    assert_eq!(
        status(&repo).unwrap(),
        Status::default(),
        "the changes were put aside"
    );
    let stashes = stashes(&repo).unwrap();
    assert_eq!(stashes.len(), 1);
    let stash = &stashes[0];
    assert_eq!(
        (stash.name.as_str(), stash.message.as_str()),
        ("stash@{0}", "On main: try this")
    );
    assert_eq!(stash.base, repo.sha("main"));
    assert!(stash.has_untracked);
    assert_eq!(
        stash_files(&repo, stash).unwrap(),
        [
            FileEntry {
                status: 'M',
                path: "README.md".to_string(),
                lines: Lines::Counted {
                    added: 1,
                    removed: 1
                }
            },
            FileEntry {
                status: '?',
                path: "untracked.txt".to_string(),
                lines: Lines::Counted {
                    added: 2,
                    removed: 0
                }
            },
        ]
    );
}

#[test]
fn a_stash_s_files_diff_against_its_base_or_against_nothing() {
    let repo = stashed();
    let stash = stashes(&repo).unwrap().remove(0);
    let tracked = DiffSource::Stash {
        sha: stash.sha.clone(),
        path: "README.md".to_string(),
        untracked: false,
    };
    assert!(
        diff(&repo, &tracked, false)
            .unwrap()
            .contains("-More.\n+Stashed.\n")
    );
    let versions = file_versions(&repo, &tracked);
    assert_eq!(
        versions,
        (
            Some("# Test\n\nMore.\n".to_string()),
            Some("# Test\n\nStashed.\n".to_string())
        )
    );
    let untracked = DiffSource::Stash {
        sha: stash.sha,
        path: "untracked.txt".to_string(),
        untracked: true,
    };
    assert!(
        diff(&repo, &untracked, false)
            .unwrap()
            .contains("+new\n+file\n")
    );
    assert_eq!(
        file_versions(&repo, &untracked),
        (None, Some("new\nfile\n".to_string()))
    );
}

#[test]
fn a_stash_without_untracked_files_lists_only_its_changes() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "changed\n");
    repo.git(&["stash", "-q"]);
    let stash = stashes(&repo).unwrap().remove(0);
    assert!(!stash.has_untracked);
    assert_eq!(paths(&stash_files(&repo, &stash).unwrap()), ["README.md"]);
}

#[test]
fn applying_keeps_the_stash_and_popping_drops_it() {
    let repo = stashed();
    let stash = stashes(&repo).unwrap().remove(0);
    stash_apply(&repo, &stash).unwrap();
    assert_eq!(
        repo.read("README.md").as_deref(),
        Some("# Test\n\nStashed.\n")
    );
    assert_eq!(repo.read("untracked.txt").as_deref(), Some("new\nfile\n"));
    assert_eq!(shas(&stashes(&repo).unwrap()), [stash.sha.as_str()]);
    // Everything comes back unstaged.
    assert!(status(&repo).unwrap().staged.is_empty());

    discard_all(
        &repo,
        &["README.md".to_string()],
        &["untracked.txt".to_string()],
    )
    .unwrap();
    stash_pop(&repo, &stash).unwrap();
    assert_eq!(
        repo.read("README.md").as_deref(),
        Some("# Test\n\nStashed.\n")
    );
    assert!(stashes(&repo).unwrap().is_empty());
}

#[test]
fn a_stash_is_found_by_its_sha_after_its_name_shifts() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "older\n");
    repo.git(&["stash", "-q"]);
    let older = stashes(&repo).unwrap().remove(0);
    assert_eq!(older.name, "stash@{0}");
    // A newer stash makes the older one stash@{1}.
    repo.write("README.md", "newer\n");
    repo.git(&["stash", "-q"]);
    stash_drop(&repo, &older).unwrap();
    let left = stashes(&repo).unwrap();
    assert_eq!(left.len(), 1);
    assert_ne!(left[0].sha, older.sha, "the newer one is kept");
    // Once gone, it says so rather than drop another.
    let error = stash_drop(&repo, &older).unwrap_err().to_string();
    assert_eq!(error, "stash@{0} is gone: it was dropped or popped");
    assert_eq!(stashes(&repo).unwrap().len(), 1);
}

#[test]
fn a_pop_that_meets_a_conflict_keeps_the_stash() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "stashed\n");
    repo.git(&["stash", "-q"]);
    let stash = stashes(&repo).unwrap().remove(0);
    repo.write("README.md", "committed since\n");
    repo.commit("Change the README");
    assert!(stash_pop(&repo, &stash).is_err());
    assert_eq!(shas(&stashes(&repo).unwrap()), [stash.sha.as_str()]);
    assert_eq!(status(&repo).unwrap().conflicted, [entry('U', "README.md")]);
}

// --------------------------------------------------------------- branches

/// A clone of a repo with history, whose `main` has a commit of its own and
/// is a commit behind its origin's, fetched.
fn diverged_clone() -> (TestRepo, TestRepo) {
    let origin = TestRepo::with_history();
    let clone = origin.clone_of();
    origin.write("from-origin.txt", "o\n");
    origin.commit("On origin");
    clone.write("local.txt", "l\n");
    clone.commit("Local");
    clone.git(&["fetch", "-q"]);
    (origin, clone)
}

#[test]
fn branches_know_their_upstream_and_how_far_apart_they_are() {
    let (_origin, clone) = diverged_clone();
    let branches = branches(&clone).unwrap();
    // origin/HEAD, only a pointer, is left out.
    assert_eq!(names(&branches), ["main", "origin/feature", "origin/main"]);
    let main = &branches[0];
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!(main.ahead_behind, Some((1, 1)));
    assert!(!main.is_remote && branches[1].is_remote);
    assert_eq!(branches[1].local_name, "feature");
    assert_eq!(tracked_remotes(&clone), ["origin"]);
    // A branch without an upstream has no counts.
    clone.git(&["branch", "-q", "loose"]);
    let loose = super::branches(&clone)
        .unwrap()
        .into_iter()
        .find(|b| b.name == "loose")
        .unwrap();
    assert_eq!((loose.upstream, loose.ahead_behind), (None, None));
}

#[test]
fn a_branch_whose_upstream_was_deleted_has_no_counts() {
    let (origin, clone) = diverged_clone();
    clone.git(&["switch", "-q", "--track", "origin/feature"]);
    origin.git(&["branch", "-q", "-D", "feature"]);
    clone.git(&["fetch", "-q", "--prune"]);
    let feature = branches(&clone)
        .unwrap()
        .into_iter()
        .find(|b| b.name == "feature")
        .unwrap();
    assert_eq!(feature.upstream.as_deref(), Some("origin/feature"));
    assert_eq!(feature.ahead_behind, None);
}

#[test]
fn unpushed_commits_are_the_ones_on_no_remote_branch() {
    let (_origin, clone) = diverged_clone();
    let unpushed = unpushed(&clone, &["main".to_string()]).unwrap();
    assert_eq!(unpushed, HashSet::from([clone.sha("main")]));
    assert!(
        super::unpushed(&clone, &["origin/main".to_string()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn the_background_fetch_brings_the_remote_s_new_commits() {
    let origin = TestRepo::with_history();
    let clone = origin.clone_of();
    origin.write("later.txt", "later\n");
    let new = origin.commit("Later");
    assert_ne!(resolve(&clone, "origin/main"), Some(new.clone()));
    background_fetch(&clone, &tracked_remotes(&clone)).unwrap();
    assert_eq!(resolve(&clone, "origin/main"), Some(new));
    // It leaves FETCH_HEAD to the user's own fetches.
    assert!(!clone.git_dir.join("FETCH_HEAD").exists());
    // With nothing to fetch from, there's nothing to do.
    background_fetch(&origin, &tracked_remotes(&origin)).unwrap();
}

#[test]
fn a_failed_fetch_says_why_in_a_line() {
    let repo = TestRepo::with_history();
    repo.git(&[
        "remote",
        "add",
        "gone",
        &repo.file("no-such-repo").display().to_string(),
    ]);
    let error = background_fetch(&repo, &["gone".to_string()])
        .unwrap_err()
        .to_string();
    assert!(!error.is_empty() && !error.contains('\n'), "{error}");
}

#[test]
fn the_default_view_is_the_branch_and_its_remote() {
    let (_origin, clone) = diverged_clone();
    assert_eq!(default_refs(&clone), ["main", "origin/main"]);
    // An upstream with another name is the one shown.
    clone.git(&["switch", "-q", "-c", "mine", "--track", "origin/feature"]);
    assert_eq!(default_refs(&clone), ["mine", "origin/feature"]);
    // Without an upstream, origin's branch of the same name, if any.
    let alone = TestRepo::with_history();
    assert_eq!(default_refs(&alone), ["main", "origin/main"]);
    assert_eq!(resolve_all(&alone, &default_refs(&alone))[1], None);
}

#[test]
fn a_detached_head_is_on_no_branch() {
    let repo = TestRepo::with_history();
    repo.git(&["switch", "-q", "--detach", "main~1"]);
    assert_eq!(current_branch(&repo), None);
    assert_eq!(default_refs(&repo), ["HEAD"]);
    assert_eq!(resolve(&repo, "HEAD"), Some(repo.sha("main~1")));
}

#[test]
fn switching_checks_out_a_branch_or_tracks_a_remote_one() {
    let (_origin, clone) = diverged_clone();
    switch_track(&clone, "origin/feature").unwrap();
    assert_eq!(current_branch(&clone).as_deref(), Some("feature"));
    let feature = branches(&clone)
        .unwrap()
        .into_iter()
        .find(|b| b.name == "feature")
        .unwrap();
    assert_eq!(feature.upstream.as_deref(), Some("origin/feature"));
    switch(&clone, "main").unwrap();
    assert_eq!(current_branch(&clone).as_deref(), Some("main"));
    assert!(switch(&clone, "no-such-branch").is_err());
}

#[test]
fn switching_with_changes_in_the_way_fails_and_says_why() {
    let repo = TestRepo::with_history();
    repo.git(&["switch", "-q", "feature"]);
    repo.write("feature.txt", "changed\n");
    let error = switch(&repo, "main").unwrap_err().to_string();
    assert!(error.contains("feature.txt"), "{error}");
    assert_eq!(current_branch(&repo).as_deref(), Some("feature"));
}

#[test]
fn a_branch_in_another_worktree_says_where_it_is() {
    let repo = TestRepo::with_history();
    let elsewhere = repo.root().with_file_name(format!(
        "{}-wt",
        repo.root().file_name().unwrap().to_string_lossy()
    ));
    repo.git(&[
        "worktree",
        "add",
        "-q",
        &elsewhere.display().to_string(),
        "feature",
    ]);
    let branches = branches(&repo).unwrap();
    assert_eq!(names(&branches), ["feature", "main"]);
    let (feature, main) = (&branches[0], &branches[1]);
    let worktree = |branch: &Branch| other_worktree(&repo, branch.worktree.as_deref()?);
    let name = elsewhere.file_name().unwrap().to_string_lossy();
    assert_eq!(worktree(feature), Some(format!("../{name}")));
    assert_eq!(worktree(main), None, "main is checked out here");
    // ...and the worktree, as a repo of its own, reads its own HEAD.
    let other = Repo::discover(&elsewhere).unwrap();
    assert_eq!(current_branch(&other).as_deref(), Some("feature"));
    assert_eq!(status(&other).unwrap(), Status::default());
    let _ = std::fs::remove_dir_all(&elsewhere);
}

#[test]
fn branches_holding_a_commit_are_found_by_its_sha() {
    let (_origin, clone) = diverged_clone();
    let first = clone.sha("main~3");
    let holding = branches_containing(&clone, short(&first)).unwrap();
    assert_eq!(holding, ["main", "origin/feature", "origin/main"]);
    let only_feature = branches_containing(&clone, &clone.sha("origin/feature")).unwrap();
    assert_eq!(only_feature, ["origin/feature"]);
    assert_eq!(branches_containing(&clone, "abcdef1234"), None);
}

#[test]
fn tags_are_listed_by_name() {
    let repo = TestRepo::with_history();
    repo.git(&["tag", "v1"]);
    repo.git(&["tag", "-a", "v2", "-m", "Second", "main~1"]);
    assert_eq!(tags(&repo).unwrap(), ["v1", "v2"]);
    // An annotated tag resolves to its commit.
    assert_eq!(resolve(&repo, "v2"), Some(repo.sha("main~1")));
}

// ---------------------------------------------------------------- history

#[test]
fn the_log_lists_children_before_parents_with_each_one_s_parents() {
    let repo = TestRepo::with_history();
    repo.git(&["merge", "-q", "--no-ff", "-m", "Merge feature", "feature"]);
    let commits = log(&repo, &["main".to_string()], 100).unwrap();
    let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(subjects.len(), 5);
    assert_eq!(subjects[0], "Merge feature");
    assert_eq!(subjects[4], "First");
    assert_eq!(
        commits[0].parents,
        [repo.sha("main^1"), repo.sha("feature")]
    );
    assert!(commits[4].parents.is_empty());
    assert!(commits.iter().all(|commit| commit.author == "Test"));
    let position = |sha: &str| commits.iter().position(|c| c.sha == sha).unwrap();
    for (index, commit) in commits.iter().enumerate() {
        assert!(
            commit.parents.iter().all(|parent| position(parent) > index),
            "{} before its parents",
            commit.subject
        );
    }
    assert_eq!(log(&repo, &["main".to_string()], 2).unwrap().len(), 2);
}

#[test]
fn a_commit_is_named_by_its_short_sha_and_subject() {
    let repo = TestRepo::with_history();
    let sha = repo.sha("main");
    assert_eq!(
        commit_label(&repo, &sha),
        Some(format!("{}  Third", short(&sha)))
    );
    assert_eq!(commit_label(&repo, "no-such-commit"), None);
    // Shas show short, and names as they are.
    assert_eq!(display_ref(&repo, &sha), short(&sha));
    assert_eq!(display_ref(&repo, "feature"), "feature");
    assert_eq!(
        display_ref(&repo, "abcdef1234"),
        "abcdef1234",
        "not a commit here"
    );
    let details = commit_details(&repo, &sha).unwrap();
    assert_eq!((details.sha, details.message), (sha, "Third".to_string()));
    assert!(commit_details(&repo, "no-such-commit").is_err());
}

#[test]
fn the_refs_fingerprint_changes_when_a_ref_or_head_moves_and_only_then() {
    let repo = TestRepo::with_history();
    let before = refs_fingerprint(&repo);
    repo.write("README.md", "changed but not committed\n");
    assert_eq!(refs_fingerprint(&repo), before);
    // Each of these moves a ref or HEAD.
    let moves = |args: &[&str]| {
        let before = refs_fingerprint(&repo);
        repo.git(args);
        assert_ne!(refs_fingerprint(&repo), before, "git {args:?}");
    };
    moves(&["commit", "-q", "-am", "A commit"]);
    moves(&["switch", "-q", "feature"]);
    // HEAD on the same commit, but detached.
    moves(&["switch", "-q", "--detach", "feature"]);
    moves(&["tag", "v1"]);
    repo.write("feature.txt", "to stash\n");
    moves(&["stash", "-q"]);
}

#[test]
fn a_commit_s_size_counts_its_files_and_lines_and_its_details_keep_its_message_whole() {
    let repo = TestRepo::with_history();
    repo.write("a.txt", "one\ntwo\nthree\n");
    repo.write("README.md", "# Test\n");
    repo.git(&["add", "."]);
    // A message that looks like a stat, which mustn't be read as one.
    repo.git(&["commit", "-qm", "Add a\n\n 1 file changed, 9 insertions(+)"]);
    let details = commit_details(&repo.repo, "HEAD").unwrap();
    assert_eq!(details.message, "Add a\n\n 1 file changed, 9 insertions(+)");
    let size = commit_size(&repo.repo, "HEAD").unwrap();
    assert_eq!((size.files, size.lines), (2, ui_total(&repo, "HEAD")));
    // A commit that changes nothing.
    repo.git(&["commit", "-q", "--allow-empty", "-m", "Nothing"]);
    assert_eq!(commit_size(&repo.repo, "HEAD").unwrap().files, 0);
    assert_eq!(
        commit_details(&repo.repo, "HEAD").unwrap().message,
        "Nothing"
    );
    assert!(commit_size(&repo.repo, "no-such-commit").is_err());
}

/// What the graph's files for a commit add up to, which the details must match.
fn ui_total(repo: &TestRepo, sha: &str) -> Lines {
    let files = commit_files(&repo.repo, sha).unwrap();
    crate::ui::total_lines(files.into_iter().map(|file| file.lines))
}
