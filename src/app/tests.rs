use super::*;
use super::layout::{accent_bar, fit_folder};
use super::popups::render_text_box;
use super::status_line::hint_spans;
use crate::test_repo::TestRepo;

/// A whole app on `repo`, which must outlive it.
fn app_on(repo: &TestRepo) -> App {
    App::new(repo.repo.clone(), PathBuf::new())
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::from(code));
}

#[test]
fn the_colour_bar_ends_in_the_repo_s_folder_and_name_as_far_as_they_fit() {
    let bar = |width| accent_bar(width, "~/Code/", "gclic");
    // 30 wide: 28 inside, of which " ~/Code/gclic" takes 13.
    assert_eq!(bar(30), ("━".repeat(15), Some(("~/Code/".to_string(), "gclic".to_string()))));
    // Shorter: the folder goes before the name, and some line always shows.
    assert_eq!(bar(16).1, Some((String::new(), "gclic".to_string())));
    assert_eq!(bar(16).0.chars().count(), 8);
    assert_eq!(bar(8), ("━".repeat(6), None));
    assert_eq!(bar(1), (String::new(), None));
}

#[test]
fn the_project_shortens_its_folder_before_its_name() {
    let (folder, name) = ("~/Code/", "gclic");
    assert_eq!(fit_folder(folder, name, 40).as_deref(), Some("~/Code/"));
    assert_eq!(fit_folder(folder, name, 11).as_deref(), Some("…Code/"));
    assert_eq!(fit_folder(folder, name, 8).as_deref(), Some(""));
    assert_eq!(fit_folder(folder, name, 4), None);
}

#[test]
fn each_tab_keeps_its_own_focus() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.focus_left(Pane::Graph);
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Compare && app.view.focused(Pane::Compare));
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Repo && app.view.focused(Pane::Graph));
}

#[test]
fn h_and_l_still_move_on_the_repo_tab_with_a_comparison_kept() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.start_compare("HEAD~1".to_string(), "HEAD".to_string());
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Repo && app.compare.is_some());
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('l'));
    assert!(app.view.focused(Pane::Graph));
    press(&mut app, KeyCode::Char('h'));
    assert!(app.view.focused(Pane::Changes));
}

#[test]
fn gg_goes_to_the_top_of_a_picker_and_a_single_g_does_not() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.focus_left(Pane::Graph);
    press(&mut app, KeyCode::Char('b'));
    let row = |app: &App| match &app.modal {
        Modal::Picker(picker, _) => picker.selected_row(),
        _ => panic!("the branches picker should be open"),
    };
    press(&mut app, KeyCode::Char('G'));
    let last = row(&app);
    assert!(last > 0);
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(row(&app), last);
    press(&mut app, KeyCode::Char('g'));
    assert_eq!(row(&app), 0);
    // A g, then anything else, does nothing.
    press(&mut app, KeyCode::Char('G'));
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('k'));
    assert_eq!(row(&app), last);
}

#[test]
fn the_repo_is_read_often_while_in_use_and_rarely_when_idle() {
    assert_eq!(refresh_every(Duration::ZERO), REFRESH_IN_USE);
    assert_eq!(refresh_every(IDLE_AFTER - Duration::from_millis(1)), REFRESH_IN_USE);
    assert_eq!(refresh_every(IDLE_AFTER), REFRESH_IDLE);
}

#[test]
fn hints_show_keys_in_the_key_colour() {
    let spans = hint_spans(&[("space", "stage"), ("a/A", "stage/unstage all"), ("", "type to filter")]);
    let text: Vec<(&str, Option<Color>)> = spans.iter().map(|s| (s.content.as_ref(), s.style.fg)).collect();
    assert_eq!(
        text,
        [
            ("space", Some(ui::KEY)),
            (" stage", None),
            (" · ", Some(ui::DIM)),
            ("a/A", Some(ui::KEY)),
            (" stage/unstage all", None),
            (" · ", Some(ui::DIM)),
            ("type to filter", None),
        ]
    );
}

/// Fails if a key on the status line isn't in the help, where the user is
/// or among the keys that work everywhere or on any diff.
fn assert_hints_in_help(app: &App) {
    let mut titles = vec![app.help_section(), "Everywhere", "Diff"];
    if app.compare.as_ref().is_some_and(|compare| compare.stash_shown().is_some()) {
        titles.push("Stashes");
    }
    for (key, action) in app.hints() {
        assert!(key.is_empty() || crate::help::lists(&titles, key), "{key} {action} isn't in the help's {titles:?}");
    }
}

#[test]
fn every_hint_is_in_the_help() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    for pane in [Pane::Branches, Pane::Changes, Pane::Graph] {
        app.focus_left(pane);
        assert_hints_in_help(&app);
    }
    // A commit expanded, one of its files, its diff, then inside the diff,
    // and a commit's details.
    for code in [KeyCode::Enter, KeyCode::Char('j'), KeyCode::Enter, KeyCode::Enter] {
        press(&mut app, code);
        assert_hints_in_help(&app);
    }
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('i'));
    assert_hints_in_help(&app);
    press(&mut app, KeyCode::Char('i'));
    assert_hints_in_help(&app);
    // The Compare tab, empty and then with a comparison and its diff.
    press(&mut app, KeyCode::Tab);
    assert_hints_in_help(&app);
    app.start_compare("HEAD~1".to_string(), "HEAD".to_string());
    assert_hints_in_help(&app);
    press(&mut app, KeyCode::Enter);
    assert_hints_in_help(&app);
}

#[test]
fn confirming_the_default_branches_keeps_the_default_view() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    let shown = app.graph.shown_refs();
    assert!(app.graph.is_default_view() && !shown.is_empty());
    app.use_picked(PickerUse::GraphBranches, shown.into_iter().rev().collect());
    assert!(app.graph.is_default_view());
    // Picking something else makes a custom view.
    app.use_picked(PickerUse::GraphBranches, vec!["HEAD~1".to_string()]);
    assert!(!app.graph.is_default_view());
}

#[test]
fn esc_on_an_empty_compare_tab_goes_back_to_repo() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);
    assert!(app.tab == Tab::Repo);
}

#[test]
fn c_on_the_compare_tab_asks_for_refs() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.modal, Modal::Picker(_, PickerUse::CompareA)));
}

#[test]
fn starting_a_comparison_switches_to_its_tab() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.start_compare("HEAD~1".to_string(), "HEAD".to_string());
    assert!(app.tab == Tab::Compare && app.view.focused(Pane::Compare));
    assert!(app.compare.as_ref().is_some_and(|c| c.refs() == "HEAD~1 → HEAD"));
    // Closing it asks first, and esc cancels.
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.modal, Modal::Confirm(Confirm::CloseCompare)));
    press(&mut app, KeyCode::Esc);
    assert!(app.tab == Tab::Compare && app.compare.is_some());
    // Enter closes it, going back to where the user was.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Enter);
    assert!(app.tab == Tab::Repo && app.compare.is_none());
}

#[test]
fn closing_a_comparison_from_the_repo_tab_leaves_the_repo_tab_alone() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.start_compare("HEAD~1".to_string(), "HEAD".to_string());
    app.show_diff(last_commit_diff());
    press(&mut app, KeyCode::Tab);
    app.show_diff(last_commit_diff());
    // As popping the stash shown on the Compare tab does, from the graph.
    app.close_compare();
    assert!(app.tab == Tab::Repo && app.compare.is_none() && app.view.diff().is_some());
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Compare && app.view.focused(Pane::Compare) && app.view.diff().is_none());
}

fn type_text(app: &mut App, text: &str) {
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
}

#[test]
fn a_comment_opens_the_review_tab_and_deleting_the_last_closes_it() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    let source = last_commit_diff();
    app.show_diff(source.clone());
    app.focus_preview();
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(app.tabs().len(), 2);

    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.modal, Modal::CommentBox(_)));
    type_text(&mut app, "Why?");
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(app.review.len(), 1);
    assert!(app.review.text().contains("> Why?"));
    assert!(app.tabs().contains(&Tab::Review));

    // gc again edits the same comment.
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(&app.modal, Modal::CommentBox(comment) if comment.draft == "Why?"));
    press(&mut app, KeyCode::Esc);

    // Tab leads to Compare, then Review, which shows the comment's diff.
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Review && app.view.focused(Pane::Review));
    assert!(app.view.diff().is_some_and(|diff| diff.source == source));

    // Deleting asks first, and esc keeps it.
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.review.len(), 1);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert!(app.review.is_empty());
    assert!(app.tab == Tab::Repo && !app.tabs().contains(&Tab::Review));
}

/// The diff of the file the last commit of `TestRepo::with_history` changed.
fn last_commit_diff() -> DiffSource {
    DiffSource::Refs { a: "HEAD~1".to_string(), b: "HEAD".to_string(), path: "README.md".to_string() }
}

#[test]
fn e_switches_the_diffs_on_every_tab() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.show_diff(last_commit_diff());
    app.start_compare("HEAD~1".to_string(), "HEAD".to_string());
    app.show_diff(last_commit_diff());
    press(&mut app, KeyCode::Char('e'));
    assert!(app.whole && app.view.diff().is_some_and(|diff| diff.whole));
    // The Repo tab's diff, put aside, shows whole once its tab does.
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Repo && app.view.diff().is_some_and(|diff| diff.whole));
    press(&mut app, KeyCode::Char('e'));
    press(&mut app, KeyCode::Tab);
    assert!(app.view.diff().is_some_and(|diff| !diff.whole));
}

#[test]
fn gg_goes_to_the_top_and_g_then_another_key_does_nothing() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.show_diff(last_commit_diff());
    app.focus_preview();
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('g'));
    assert!(app.pending_g);
    press(&mut app, KeyCode::Char('g'));
    assert!(!app.pending_g && app.view.diff().unwrap().active_anchor().is_none(), "on the first heading");
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Esc);
    assert!(!app.pending_g && app.view.in_preview);
}

#[test]
fn gc_from_a_list_needs_n_to_show_the_active_line_and_gc_upper_takes_the_file() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.focus_left(Pane::Graph);
    app.show_diff(last_commit_diff());
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.modal, Modal::None));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(&app.modal, Modal::CommentBox(comment) if comment.anchor.is_some()));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('C'));
    assert!(matches!(&app.modal, Modal::CommentBox(comment) if comment.anchor.is_none()));
    type_text(&mut app, "Split it.");
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert_eq!(app.review.len(), 1);
    assert!(app.review.find(&app.view.diff().unwrap().source, None).is_some());
}

#[test]
fn a_long_draft_keeps_its_end_in_view() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let draft: String = (1..=40).map(|n| format!("line {n}\n")).collect::<String>() + "the end";
    let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
    terminal
        .draw(|frame| render_text_box(frame, frame.area(), " Box ", None, &draft, Line::default()))
        .unwrap();
    let screen: String = terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).collect();
    assert!(screen.contains("the end▏"));
    assert!(!screen.contains("line 1 "), "the start scrolled away");
}

#[test]
fn clearing_the_review_asks_first() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    let anchor = Some(Anchor { side: review::Side::New, number: 1, text: "x".to_string() });
    let source = DiffSource::Unstaged("a.rs".to_string());
    app.review.set(Comment::new(&app.repo, source, anchor, "y".to_string()).unwrap());
    app.go_to_tab(Tab::Review);
    press(&mut app, KeyCode::Char('D'));
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.review.len(), 1);
    press(&mut app, KeyCode::Char('D'));
    press(&mut app, KeyCode::Enter);
    assert!(app.review.is_empty() && app.tab == Tab::Repo);
}

fn yanks(labels: &[&str]) -> Vec<String> {
    let mut yanks = Vec::new();
    for label in labels {
        remember_yank(&mut yanks, label.to_string());
    }
    yanks
}

#[test]
fn yanks_keep_their_order() {
    assert_eq!(yanks(&["aaa  one", "bbb  two"]), ["aaa  one", "bbb  two"]);
}

#[test]
fn only_the_last_two_yanks_are_kept() {
    assert_eq!(yanks(&["aaa  one", "bbb  two", "ccc  three"]), ["bbb  two", "ccc  three"]);
}

#[test]
fn yanking_a_commit_again_moves_it_to_the_end() {
    assert_eq!(yanks(&["aaa  one", "bbb  two", "aaa  one"]), ["bbb  two", "aaa  one"]);
}

// These set up what they test in a repo of their own, as the user would
// meet it, and drive the app with keys.

/// The screen as the app draws it, as text.
fn screen(app: &mut App) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let cells: Vec<&str> = buffer.content().iter().map(|cell| cell.symbol()).collect();
    cells.chunks(width).map(|row| row.concat() + "\n").collect()
}

fn ctrl(app: &mut App, ch: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL));
}

/// A repo with one stash, of a change to the README and a new file.
fn stashed() -> TestRepo {
    let repo = TestRepo::with_history();
    repo.write("README.md", "# Test\n\nStashed.\n");
    repo.write("untracked.txt", "new\n");
    repo.git(&["stash", "push", "-q", "--include-untracked", "-m", "try this"]);
    repo
}

#[test]
fn an_empty_repo_opens_without_an_error() {
    let repo = TestRepo::new();
    let mut app = app_on(&repo);
    assert!(matches!(app.modal, Modal::None), "{}", app.message);
    let shown = screen(&mut app);
    assert!(shown.contains("Changes") && shown.contains("Graph"), "{shown}");
    // Its first files can be staged and committed.
    repo.write("first.txt", "first\n");
    app.refresh();
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('c'));
    type_text(&mut app, "First");
    ctrl(&mut app, 's');
    assert_eq!(app.message, "Committed");
    assert_eq!(git::commit_label(&repo, "HEAD").map(|label| label.ends_with("  First")), Some(true));
}

#[test]
fn a_file_is_staged_and_committed_from_the_changes_pane() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "# Test\n\nEdited.\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    assert!(screen(&mut app).contains("Changes (1)"));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(git::status(&repo).unwrap().staged.len(), 1);
    assert!(screen(&mut app).contains("Staged (1)"));
    // An empty message isn't committed.
    press(&mut app, KeyCode::Char('c'));
    ctrl(&mut app, 's');
    assert_eq!(app.message, "The commit message is empty");
    type_text(&mut app, "Edit the README");
    ctrl(&mut app, 's');
    assert_eq!(app.message, "Committed");
    assert_eq!(git::commit_details(&repo, "HEAD").unwrap().message, "Edit the README");
    assert_eq!(git::status(&repo).unwrap(), git::Status::default());
    // With nothing staged, c says so rather than open the box.
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.modal, Modal::None));
    assert_eq!(app.message, "Nothing staged to commit");
}

#[test]
fn a_failed_commit_keeps_its_message() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "edited\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char(' '));
    // A hook that refuses every commit.
    repo.write("hooks/pre-commit", "#!/bin/sh\necho refused >&2\nexit 1\n");
    repo.git(&["config", "core.hooksPath", &repo.file("hooks").display().to_string()]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(repo.file("hooks/pre-commit"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    press(&mut app, KeyCode::Char('c'));
    type_text(&mut app, "Refused");
    ctrl(&mut app, 's');
    assert!(matches!(&app.modal, Modal::Error(error) if error.contains("refused")), "{:?}", app.message);
    assert_eq!(app.commit_draft, "Refused");
    assert_eq!(git::commit_label(&repo, "HEAD").map(|label| label.ends_with("  Third")), Some(true));
}

#[test]
fn restoring_a_file_asks_first() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "edited\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(&app.modal, Modal::Confirm(Confirm::Restore(path)) if path == "README.md"));
    press(&mut app, KeyCode::Esc);
    assert_eq!(repo.read("README.md").as_deref(), Some("edited\n"));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(repo.read("README.md").as_deref(), Some("# Test\n\nMore.\n"));
    assert!(screen(&mut app).contains("No changes"));
}

#[test]
fn discarding_everything_asks_first_and_takes_new_files_too() {
    let repo = TestRepo::with_history();
    repo.write("README.md", "edited\n");
    repo.write("new.txt", "new\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('D'));
    assert!(matches!(app.modal, Modal::Confirm(Confirm::DiscardAll { .. })));
    press(&mut app, KeyCode::Enter);
    assert_eq!(git::status(&repo).unwrap(), git::Status::default());
    assert_eq!(repo.read("new.txt"), None);
}

#[test]
fn a_merge_conflict_shows_in_the_changes_pane_with_both_sides() {
    let repo = TestRepo::with_history();
    repo.conflicting_merge("clash.txt");
    let mut app = app_on(&repo);
    let shown = screen(&mut app);
    assert!(shown.contains("merging"), "{shown}");
    assert!(shown.contains("Conflicts (1)"), "{shown}");
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Enter);
    let diff = app.view.diff().expect("the conflict's diff");
    assert_eq!(diff.source, DiffSource::Conflict("clash.txt".to_string()));
    assert!(diff.text().contains("<<<<<<< HEAD"));
    assert!(screen(&mut app).contains("conflict: yours ↔ theirs"));
    // Staging it marks it resolved.
    repo.write("clash.txt", "both\n");
    press(&mut app, KeyCode::Char(' '));
    assert!(git::status(&repo).unwrap().conflicted.is_empty());
}

#[test]
fn a_conflict_met_while_rebasing_names_its_sides_the_other_way() {
    let repo = TestRepo::with_history();
    repo.conflicting_merge("clash.txt");
    repo.git(&["merge", "--abort"]);
    repo.git(&["switch", "-q", "other"]);
    assert!(!repo.git_may_fail(&["rebase", "main"]));
    let mut app = app_on(&repo);
    assert!(screen(&mut app).contains("rebasing"));
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("conflict: upstream ↔ your commit"));
}

#[test]
fn popping_the_stash_shown_on_the_compare_tab_closes_it_from_the_repo_tab() {
    // #55, as it happened: a stash open on the Compare tab, popped from the
    // graph on the Repo tab.
    let repo = stashed();
    let mut app = app_on(&repo);
    let stash = git::stashes(&repo).unwrap().remove(0);
    app.open_stash(stash.clone());
    assert!(app.tab == Tab::Compare && app.compare.as_ref().and_then(|c| c.stash_shown()) == Some(&stash));
    press(&mut app, KeyCode::Tab);
    app.focus_left(Pane::Graph);
    // The stash sits above its base, the newest commit, at the top.
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('p'));
    assert_eq!(app.message, "Popped stash@{0}");
    assert!(git::stashes(&repo).unwrap().is_empty());
    assert_eq!(repo.read("README.md").as_deref(), Some("# Test\n\nStashed.\n"));
    assert!(app.tab == Tab::Repo && app.compare.is_none());
    assert!(screen(&mut app).contains("Changes (2)"));
    // The Compare tab is empty, ready for another comparison.
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Compare && app.view.diff().is_none());
}

#[test]
fn dropping_a_stash_asks_first() {
    let repo = stashed();
    let mut app = app_on(&repo);
    let stash = git::stashes(&repo).unwrap().remove(0);
    app.open_stash(stash);
    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.modal, Modal::Confirm(Confirm::DropStash(_))));
    press(&mut app, KeyCode::Esc);
    assert_eq!(git::stashes(&repo).unwrap().len(), 1);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.message, "Dropped stash@{0}");
    assert!(git::stashes(&repo).unwrap().is_empty());
    // Dropping puts nothing back.
    assert_eq!(git::status(&repo).unwrap(), git::Status::default());
    assert!(app.compare.is_none());
}

#[test]
fn applying_a_stash_keeps_it_and_its_view() {
    let repo = stashed();
    let mut app = app_on(&repo);
    let stash = git::stashes(&repo).unwrap().remove(0);
    app.open_stash(stash.clone());
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(app.message, "Applied stash@{0}");
    assert_eq!(git::stashes(&repo).unwrap()[0].sha, stash.sha);
    assert_eq!(repo.read("untracked.txt").as_deref(), Some("new\n"));
    assert!(app.tab == Tab::Compare && app.compare.is_some());
}

#[test]
fn a_stash_s_files_open_in_its_view() {
    let repo = stashed();
    let mut app = app_on(&repo);
    app.open_stash(git::stashes(&repo).unwrap().remove(0));
    let shown = screen(&mut app);
    assert!(shown.contains("README.md") && shown.contains("untracked.txt"), "{shown}");
    press(&mut app, KeyCode::Enter);
    let diff = app.view.diff().expect("the first file's diff");
    assert!(diff.text().contains("+Stashed."), "{}", diff.text());
}

#[test]
fn the_stash_command_lists_the_stashes_or_says_there_are_none() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.open_stash_picker();
    assert!(matches!(app.modal, Modal::None));
    assert!(app.message.starts_with("No stashes"));
    repo.write("README.md", "changed\n");
    repo.git(&["stash", "-q"]);
    app.open_stash_picker();
    assert!(matches!(&app.modal, Modal::Picker(_, PickerUse::Stash(stashes)) if stashes.len() == 1));
}

#[test]
fn the_branches_pane_checks_out_the_branch_picked() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.focus_left(Pane::Branches);
    let shown = screen(&mut app);
    assert!(shown.contains("feature") && shown.contains("main"), "{shown}");
    press(&mut app, KeyCode::Char('i'));
    type_text(&mut app, "feat");
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Enter);
    assert_eq!(git::current_branch(&repo).as_deref(), Some("feature"));
    // The graph follows.
    assert_eq!(app.graph.shown_refs(), ["feature"]);
}

#[test]
fn comparing_two_branches_shows_what_one_has_that_the_other_lacks() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.start_compare("main".to_string(), "feature".to_string());
    assert!(matches!(app.modal, Modal::None));
    let shown = screen(&mut app);
    assert!(shown.contains("feature.txt"), "{shown}");
    press(&mut app, KeyCode::Enter);
    assert!(app.view.diff().is_some_and(|diff| diff.text().contains("+new")));
    // A ref that isn't there says so.
    app.start_compare("main".to_string(), "no-such-branch".to_string());
    assert!(matches!(app.modal, Modal::Error(_)));
}

#[test]
fn changes_made_outside_gclic_show_after_a_refresh() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    assert!(screen(&mut app).contains("No changes"));
    repo.write("elsewhere.txt", "made in another terminal\n");
    repo.commit("Made elsewhere");
    repo.write("README.md", "and an edit\n");
    app.refresh();
    let shown = screen(&mut app);
    assert!(shown.contains("Changes (1)") && shown.contains("Made elsewhere"), "{shown}");
}

// :cd, between repos in a folder made for the test.

use crate::test_repo::TestFolder;

/// An app on `alpha`, in a folder that also holds `beta`, which has a file
/// of its own waiting to be committed.
fn two_repos() -> (TestFolder, TestRepo, TestRepo) {
    let folder = TestFolder::new();
    let (alpha, beta) = (folder.repo("alpha"), folder.repo("beta"));
    beta.write("only-in-beta.txt", "beta\n");
    (folder, alpha, beta)
}

fn app_in(folder: &TestFolder, repo: &TestRepo) -> App {
    App::new(repo.repo.clone(), folder.path.clone())
}

/// The picker open, and the repos it offers.
fn repo_picker(app: &App) -> (&Picker, &Vec<(String, PathBuf)>) {
    match &app.modal {
        Modal::Picker(picker, PickerUse::Repo(repos)) => (picker, repos),
        _ => panic!("the repo finder should be open"),
    }
}

#[test]
fn cd_with_a_name_moves_to_that_repo_and_starts_afresh() {
    let (folder, alpha, beta) = two_repos();
    let mut app = app_in(&folder, &alpha);
    app.split = Some(false);
    app.fetcher.auto = false;
    app.start_compare("main".to_string(), "feature".to_string());
    app.change_repo("beta");
    assert_eq!(app.repo, beta.repo);
    let shown = screen(&mut app);
    assert!(shown.contains("only-in-beta.txt") && shown.contains("beta"), "{shown}");
    // What was read from alpha goes, and how things are shown stays.
    assert!(app.compare.is_none() && app.tab == Tab::Repo);
    assert_eq!((app.split, app.fetcher.auto), (Some(false), false));
    assert_eq!(app.search_root, folder.path);
    // And from now on, what's done is done in beta.
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(git::status(&beta).unwrap().staged.len(), 1);
    assert_eq!(git::status(&alpha).unwrap(), git::Status::default());
}

#[test]
fn cd_alone_opens_the_finder_on_the_repo_gclic_is_in() {
    let (folder, alpha, _beta) = two_repos();
    let mut app = app_in(&folder, &alpha);
    app.change_repo("");
    let (picker, repos) = repo_picker(&app);
    let labels: Vec<&str> = repos.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels, [crate::repos::UP, "alpha", "beta"]);
    assert_eq!(picker.highlighted().map(String::as_str), Some("alpha"));
}

#[test]
fn picking_a_repo_in_the_finder_moves_there() {
    let (folder, alpha, beta) = two_repos();
    let mut app = app_in(&folder, &alpha);
    app.change_repo("");
    let Modal::Picker(_, PickerUse::Repo(repos)) = std::mem::replace(&mut app.modal, Modal::None) else {
        panic!("the repo finder should be open");
    };
    app.use_picked(PickerUse::Repo(repos), vec!["beta".to_string()]);
    assert_eq!(app.repo, beta.repo);
}

#[test]
fn cd_up_searches_a_folder_up_from_then_on() {
    let (folder, alpha, _beta) = two_repos();
    let mut app = app_in(&folder, &alpha);
    app.change_repo("..");
    assert_eq!(Some(app.search_root.as_path()), folder.path.parent());
    assert!(repo_picker(&app).0.highlighted().is_some());
}

#[test]
fn cd_with_a_name_several_repos_match_opens_the_finder_with_it_typed() {
    let folder = TestFolder::new();
    let (one, _two) = (folder.repo("app-one"), folder.repo("app-two"));
    let mut app = app_in(&folder, &one);
    app.change_repo("app");
    assert_eq!(repo_picker(&app).0.query(), "app");
    assert_eq!(app.repo, one.repo);
}

#[test]
fn cd_with_a_name_no_repo_has_says_so_and_stays() {
    let (folder, alpha, _beta) = two_repos();
    let mut app = app_in(&folder, &alpha);
    app.change_repo("gamma");
    assert!(app.message.starts_with("No repo matches gamma"), "{}", app.message);
    assert_eq!(app.repo, alpha.repo);
}

#[test]
fn a_folder_that_isnt_a_repo_is_not_opened() {
    let (folder, alpha, _beta) = two_repos();
    let mut app = app_in(&folder, &alpha);
    app.switch_repo(&folder.path, true);
    assert!(matches!(&app.modal, Modal::Error(error) if error.contains("isn't a git repo")));
    assert_eq!(app.repo, alpha.repo);
}

#[test]
fn following_neovim_moves_to_the_repo_it_went_to_once() {
    let (folder, alpha, beta) = two_repos();
    let mut app = app_in(&folder, &alpha);
    let file = folder.path.join("follow");
    app.follow = Some(editor::Follow::at(&file));
    assert!(!app.follow_neovim(), "nothing written yet");
    // A folder inside beta leads to beta.
    std::fs::write(&file, beta.file("src").display().to_string()).unwrap();
    assert!(app.follow_neovim());
    assert_eq!(app.repo, beta.repo);
    assert_eq!(app.message, "Followed Neovim to beta");
    // It keeps following, without moving again for the same folder.
    assert!(app.follow.is_some() && !app.follow_neovim());
}

// Staging a change at a time, from inside a file's diff (#86).

/// Eight numbered lines, committed, as the file each test edits.
const EIGHT: &str = "1\n2\n3\n4\n5\n6\n7\n8\n";

fn repo_with(path: &str, text: &str) -> TestRepo {
    let repo = TestRepo::new();
    repo.write(path, text);
    repo.commit("Add it");
    repo
}

/// The file as the index has it.
fn staged(repo: &TestRepo, path: &str) -> String {
    repo.git(&["show", &format!(":{path}")])
}

/// Into `source`'s diff, on its first change.
fn into_diff(app: &mut App, source: DiffSource) {
    app.show_diff(source);
    app.focus_preview();
    press(app, KeyCode::Char('n'));
}

#[test]
fn space_in_a_diff_stages_the_change_the_active_line_is_in() {
    // Two changes, close enough to share a hunk.
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    assert!(app.hints().contains(&("space", "stage change")));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.message, "Staged the change");
    assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    // The other change is left, and the active line is on it.
    let diff = app.view.diff().unwrap();
    assert_eq!(diff.change_count(), 1);
    assert_eq!(diff.active_change().map(|change| change.new.into_iter().collect::<Vec<_>>()), Some(vec![6]));
    // It's the last one, so it goes as the whole file does.
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.message, "Staged f");
    assert_eq!(staged(&repo, "f"), repo.read("f").unwrap());
    assert_eq!(app.view.diff().unwrap().change_count(), 0);
}

#[test]
fn staging_a_change_and_unstaging_it_leaves_the_index_as_it_was() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n5.5\n6\n7\nEIGHT\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(staged(&repo, "f"), "1\n2\n3\n4\n5\n5.5\n6\n7\n8\n");
    // Stage the first too, then unstage the second from the staged diff.
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('g'));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n");
    into_diff(&mut app, DiffSource::Staged("f".to_string()));
    assert!(app.hints().contains(&("space", "unstage change")));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(app.message, "Unstaged the change");
    assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(staged(&repo, "f"), EIGHT);
    // The working tree was never touched.
    assert_eq!(repo.read("f").as_deref(), Some("1\nTWO\n3\n4\n5\n5.5\n6\n7\nEIGHT\n"));
}

#[test]
fn a_change_stages_the_same_in_a_whole_file_diff_and_unified() {
    for keys in [&['e'][..], &['s'][..], &['e', 's'][..]] {
        let repo = repo_with("f", EIGHT);
        repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n");
        let mut app = app_on(&repo);
        for key in keys {
            press(&mut app, KeyCode::Char(*key));
        }
        into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n6\n7\n8\n", "with {keys:?}");
    }
}

#[test]
fn changes_at_the_ends_of_a_file_stage_alone() {
    // The first line, and a last line without a newline, in a file long
    // enough for them to be separate hunks.
    let long: String = (1..=20).map(|n| format!("{n}\n")).collect();
    let repo = repo_with("f", &long);
    let edited = format!("ONE\n{}20 and no newline", &long["1\n".len()..long.len() - "20\n".len()]);
    repo.write("f", &edited);
    let mut app = app_on(&repo);
    // The last change first, then the first.
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char(' '));
    let index = staged(&repo, "f");
    assert!(index.starts_with("1\n2\n") && index.ends_with("19\n20 and no newline"), "{index:?}");
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(staged(&repo, "f"), edited);
}

#[test]
fn a_change_in_a_file_with_spaces_in_its_path_stages() {
    let repo = repo_with("a folder/a file.txt", EIGHT);
    repo.write("a folder/a file.txt", "ONE\n2\n3\n4\n5\n6\n7\nEIGHT\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("a folder/a file.txt".to_string()));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(staged(&repo, "a folder/a file.txt"), "ONE\n2\n3\n4\n5\n6\n7\n8\n");
}

#[test]
fn a_new_or_deleted_file_is_staged_whole() {
    let repo = repo_with("gone", EIGHT);
    repo.write("new", "a\nb\n");
    std::fs::remove_file(repo.file("gone")).unwrap();
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Untracked("new".to_string()));
    press(&mut app, KeyCode::Char(' '));
    into_diff(&mut app, DiffSource::Unstaged("gone".to_string()));
    press(&mut app, KeyCode::Char(' '));
    let staged = git::status(&repo).unwrap().staged;
    assert_eq!(staged.iter().map(|file| (file.status, file.path.as_str())).collect::<Vec<_>>(), [('D', "gone"), ('A', "new")]);
    // And back, from the staged diff.
    into_diff(&mut app, DiffSource::Staged("gone".to_string()));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(git::status(&repo).unwrap().staged.len(), 1);
}

#[test]
fn space_off_a_change_or_on_a_conflict_says_why_it_does_nothing() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    let mut app = app_on(&repo);
    app.show_diff(DiffSource::Unstaged("f".to_string()));
    app.focus_preview();
    // The hunk's heading, at the top, isn't a change.
    press(&mut app, KeyCode::Char(' '));
    assert!(app.message.starts_with("Not on a change"), "{}", app.message);
    assert!(!app.hints().iter().any(|(key, _)| *key == "space"));
    assert_eq!(staged(&repo, "f"), EIGHT);

    let repo = TestRepo::with_history();
    repo.conflicting_merge("clash.txt");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Conflict("clash.txt".to_string()));
    press(&mut app, KeyCode::Char(' '));
    assert!(app.message.starts_with("A conflict is staged whole"), "{}", app.message);
    assert_eq!(git::status(&repo).unwrap().conflicted.len(), 1);
}

#[test]
fn a_diff_that_no_longer_matches_the_index_stages_nothing() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    // Staged elsewhere, as from another terminal, before gclic saw it.
    repo.git(&["add", "f"]);
    press(&mut app, KeyCode::Char(' '));
    assert!(matches!(app.modal, Modal::Error(_)), "{}", app.message);
    assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n");
}

#[test]
fn space_in_a_commit_s_diff_does_nothing() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    into_diff(&mut app, last_commit_diff());
    press(&mut app, KeyCode::Char(' '));
    assert!(app.message.is_empty(), "{}", app.message);
    assert_eq!(git::status(&repo).unwrap(), git::Status::default());
}

#[test]
fn d_in_a_diff_discards_the_change_the_active_line_is_in_after_asking() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    assert!(app.hints().contains(&("d", "discard change")));
    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(&app.modal, Modal::Confirm(Confirm::RestoreChange { path, .. }) if path == "f"));
    let shown = screen(&mut app);
    assert!(shown.contains("Discard this change to f?"), "{shown}");
    // Esc keeps it.
    press(&mut app, KeyCode::Esc);
    assert_eq!(repo.read("f").as_deref(), Some("1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n"));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.message, "Discarded the change");
    assert_eq!(repo.read("f").as_deref(), Some("1\n2\n3\n4\n5\n5.5\n6\n7\n8\n"));
    // The other change is left, with the active line on it, and the index
    // was never touched.
    assert_eq!(app.view.diff().unwrap().change_count(), 1);
    assert!(app.view.diff().unwrap().active_change().is_some());
    assert_eq!(staged(&repo, "f"), EIGHT);
}

#[test]
fn discarding_a_change_leaves_what_s_staged_alone() {
    // Line 2 staged, then lines 2 and 7 changed again on disk.
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    repo.git(&["add", "f"]);
    repo.write("f", "1\nTwo\n3\n4\n5\n6\nSEVEN\n8\n");
    let mut app = app_on(&repo);
    // The edit to line 7 first, which is the second change.
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char('n'));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    // Back to what's staged, not to what was committed.
    assert_eq!(repo.read("f").as_deref(), Some("1\nTwo\n3\n4\n5\n6\n7\n8\n"));
    assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n6\n7\n8\n");
}

#[test]
fn d_on_a_file_s_only_change_or_a_new_file_asks_as_the_list_does() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    repo.write("new", "new\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(&app.modal, Modal::Confirm(Confirm::Restore(path)) if path == "f"));
    press(&mut app, KeyCode::Esc);
    into_diff(&mut app, DiffSource::Untracked("new".to_string()));
    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(&app.modal, Modal::Confirm(Confirm::DeleteUntracked(path)) if path == "new"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(repo.read("new"), None);
}

#[test]
fn a_staged_change_is_not_discarded_from_its_diff() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    repo.git(&["add", "f"]);
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Staged("f".to_string()));
    assert!(!app.hints().iter().any(|(key, _)| *key == "d"));
    press(&mut app, KeyCode::Char('d'));
    assert!(matches!(app.modal, Modal::None));
    assert!(app.message.starts_with("Staged changes are kept"), "{}", app.message);
    assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n6\n7\n8\n");
}

#[test]
fn a_file_changed_since_its_diff_loaded_discards_nothing() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\nEIGHT\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char('d'));
    // Edited again in the editor, under the change, before confirming.
    repo.write("f", "1\nTwo\n3\n4\n5\n6\n7\nEIGHT\n");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.modal, Modal::Error(_)), "{}", app.message);
    assert_eq!(repo.read("f").as_deref(), Some("1\nTwo\n3\n4\n5\n6\n7\nEIGHT\n"));
}

// u, undoing stagings and discards.

#[test]
fn u_undoes_stagings_one_step_at_a_time() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n");
    let mut app = app_on(&repo);
    assert!(!app.hints().iter().any(|(key, _)| *key == "u"));
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(staged(&repo, "f"), repo.read("f").unwrap());
    assert!(app.hints().contains(&("u", "undo")));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid staging f");
    assert_eq!(staged(&repo, "f"), "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid staging a change to f");
    assert_eq!(staged(&repo, "f"), EIGHT);
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Nothing to undo");
    // The diff shows both changes again.
    assert_eq!(app.view.diff().unwrap().change_count(), 2);
}

#[test]
fn u_undoes_the_lists_stagings_new_files_included() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "changed\n");
    repo.write("new", "new\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(git::status(&repo).unwrap().staged.len(), 2);
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid staging everything");
    let status = git::status(&repo).unwrap();
    assert!(status.staged.is_empty());
    assert_eq!(status.unstaged.iter().map(|file| file.status).collect::<String>(), "M?");
    // And unstaging, back.
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Char('A'));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid unstaging everything");
    assert_eq!(git::status(&repo).unwrap().staged.len(), 2);
}

#[test]
fn u_brings_back_a_discarded_change_or_file() {
    let repo = repo_with("f", EIGHT);
    let edited = "1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n";
    repo.write("f", edited);
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(repo.read("f").as_deref(), Some("1\n2\n3\n4\n5\n5.5\n6\n7\n8\n"));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid discarding a change to f");
    assert_eq!(repo.read("f").as_deref(), Some(edited));
    // The whole file, from the list.
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(repo.read("f").as_deref(), Some(EIGHT));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(repo.read("f").as_deref(), Some(edited));
    // The index was never touched.
    assert_eq!(staged(&repo, "f"), EIGHT);
}

#[test]
fn undoing_discard_all_brings_back_the_changes_but_not_the_untracked_files() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "changed\n");
    repo.write("new", "new\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('D'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(git::status(&repo).unwrap(), git::Status::default());
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid discarding your changes to 1 file, though not deleting 1 untracked file");
    assert_eq!(repo.read("f").as_deref(), Some("changed\n"));
    assert_eq!(repo.read("new"), None);
}

#[test]
fn undoing_a_discarded_deletion_deletes_the_file_again() {
    let repo = repo_with("f", EIGHT);
    std::fs::remove_file(repo.file("f")).unwrap();
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(repo.read("f").as_deref(), Some(EIGHT));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(repo.read("f"), None);
}

#[test]
fn a_binary_file_comes_back_exactly() {
    let bytes = [0u8, 159, 146, 150, 13, 10, 0, 255];
    let repo = TestRepo::new();
    std::fs::write(repo.file("logo.png"), [1u8, 2, 3]).unwrap();
    repo.commit("Add a logo");
    std::fs::write(repo.file("logo.png"), bytes).unwrap();
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(std::fs::read(repo.file("logo.png")).unwrap(), bytes);
}

#[test]
fn undoing_the_staging_of_a_resolved_conflict_brings_the_conflict_back() {
    let repo = TestRepo::with_history();
    repo.conflicting_merge("clash.txt");
    repo.write("clash.txt", "both\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char(' '));
    assert!(git::status(&repo).unwrap().conflicted.is_empty());
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(git::status(&repo).unwrap().conflicted.len(), 1);
    // Both sides are there to resolve against again.
    assert_eq!(repo.git(&["show", ":3:clash.txt"]), "theirs\n");
}

#[test]
fn an_undo_that_would_lose_later_changes_is_refused_and_dropped() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n6\n7\n8\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    // f staged, then edited again and that edit discarded, from the list.
    press(&mut app, KeyCode::Char(' '));
    repo.write("f", "edited again\n");
    app.refresh();
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(repo.read("f").as_deref(), Some("1\nTWO\n3\n4\n5\n6\n7\n8\n"));
    // Then edited once more, in the editor.
    repo.write("f", "and again, by hand\n");
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Can't undo discarding your changes to f: f changed since");
    assert_eq!(repo.read("f").as_deref(), Some("and again, by hand\n"));
    // It's gone, and the step before it is next: the staging, which
    // nothing has touched since.
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid staging f");
    assert_eq!(staged(&repo, "f"), EIGHT);
}

#[test]
fn committing_clears_what_u_would_undo() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "changed\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('c'));
    type_text(&mut app, "Change f");
    ctrl(&mut app, 's');
    assert_eq!(app.message, "Committed");
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Nothing to undo");
    assert_eq!(git::status(&repo).unwrap(), git::Status::default());
}

#[test]
fn u_types_into_a_search_rather_than_undoing() {
    let repo = TestRepo::with_history();
    let mut app = app_on(&repo);
    app.focus_left(Pane::Branches);
    press(&mut app, KeyCode::Char('i'));
    press(&mut app, KeyCode::Char('u'));
    assert_ne!(app.message, "Nothing to undo");
}

#[test]
fn ctrl_r_redoes_what_u_undid() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n");
    let mut app = app_on(&repo);
    into_diff(&mut app, DiffSource::Unstaged("f".to_string()));
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    let (index, file) = (staged(&repo, "f"), repo.read("f"));
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Char('u'));
    assert_eq!((staged(&repo, "f"), repo.read("f").unwrap()), (EIGHT.to_string(), "1\nTWO\n3\n4\n5\n5.5\n6\n7\n8\n".to_string()));
    assert!(app.hints().contains(&("ctrl-r", "redo")));
    ctrl(&mut app, 'r');
    assert_eq!(app.message, "Redid staging a change to f");
    ctrl(&mut app, 'r');
    // With one change staged, the one left was the file's only one, so d
    // took the file.
    assert_eq!(app.message, "Redid discarding your changes to f");
    assert_eq!((staged(&repo, "f"), repo.read("f")), (index, file));
    ctrl(&mut app, 'r');
    assert_eq!(app.message, "Nothing to redo");
    // And undone again.
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid discarding your changes to f");
}

#[test]
fn a_new_staging_after_an_undo_leaves_nothing_to_redo() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "changed\n");
    repo.write("g", "new\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('u'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char(' '));
    assert!(!app.hints().iter().any(|(key, _)| *key == "ctrl-r"));
    ctrl(&mut app, 'r');
    assert_eq!(app.message, "Nothing to redo");
}

#[test]
fn a_redo_that_would_lose_later_changes_is_refused() {
    let repo = repo_with("f", EIGHT);
    repo.write("f", "changed\n");
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('u'));
    repo.write("f", "changed, then edited\n");
    ctrl(&mut app, 'r');
    assert_eq!(app.message, "Can't redo discarding your changes to f: f changed since");
    assert_eq!(repo.read("f").as_deref(), Some("changed, then edited\n"));
}

#[test]
fn undoing_discard_all_brings_back_every_file() {
    let repo = TestRepo::new();
    let names = ["a.txt", "b c.txt", "deep/d.txt"];
    for name in names {
        repo.write(name, "before\n");
    }
    repo.commit("Add them");
    for name in names {
        repo.write(name, &format!("{name} changed\n"));
    }
    let mut app = app_on(&repo);
    app.focus_left(Pane::Changes);
    press(&mut app, KeyCode::Char('D'));
    press(&mut app, KeyCode::Enter);
    assert_eq!(git::status(&repo).unwrap(), git::Status::default());
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(app.message, "Undid discarding your changes to 3 files");
    for name in names {
        assert_eq!(repo.read(name), Some(format!("{name} changed\n")));
    }
}
