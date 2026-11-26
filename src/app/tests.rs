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
