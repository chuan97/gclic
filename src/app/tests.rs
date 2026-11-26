use super::*;
use super::layout::{accent_bar, fit_folder};
use super::popups::render_text_box;
use super::status_line::hint_spans;

// These build a whole app on gclic's own repo.

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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
    app.focus_left(Pane::Graph);
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Compare && app.view.focused(Pane::Compare));
    press(&mut app, KeyCode::Tab);
    assert!(app.tab == Tab::Repo && app.view.focused(Pane::Graph));
}

#[test]
fn h_and_l_still_move_on_the_repo_tab_with_a_comparison_kept() {
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);
    assert!(app.tab == Tab::Repo);
}

#[test]
fn c_on_the_compare_tab_asks_for_refs() {
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.modal, Modal::Picker(_, PickerUse::CompareA)));
}

#[test]
fn starting_a_comparison_switches_to_its_tab() {
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
    // A file the last commit changed, in its diff.
    let output = std::process::Command::new("git").args(["diff", "--name-only", "HEAD~1", "HEAD"]).output().unwrap();
    let path = String::from_utf8_lossy(&output.stdout).lines().next().unwrap().to_string();
    let source = DiffSource::Refs { a: "HEAD~1".to_string(), b: "HEAD".to_string(), path };
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

/// A diff of a file the last commit changed, as `a_comment_opens…` uses.
fn last_commit_diff() -> DiffSource {
    let output = std::process::Command::new("git").args(["diff", "--name-only", "HEAD~1", "HEAD"]).output().unwrap();
    let path = String::from_utf8_lossy(&output.stdout).lines().next().unwrap().to_string();
    DiffSource::Refs { a: "HEAD~1".to_string(), b: "HEAD".to_string(), path }
}

#[test]
fn e_switches_the_diffs_on_every_tab() {
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
    let mut app = App::new(git::gclic_repo(), PathBuf::new());
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
