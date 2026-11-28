//! A fuzzy-searchable list of names. As a popup it chooses graph branches and
//! refs to compare. The branches pane uses its search and selection, and draws
//! the rows itself.

use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;

/// Higher is better. None means `query` isn't a subsequence of `item`.
pub fn fuzzy_score(query: &str, item: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0); // every item ties, so the list keeps its order
    }
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let item: Vec<char> = item.to_lowercase().chars().collect();
    let mut matched = 0;
    let mut score = 0;
    let mut previous_match = None;
    for (i, &ch) in item.iter().enumerate() {
        if matched == query.len() {
            break;
        }
        if ch == query[matched] {
            score += 1;
            if i > 0 && previous_match == Some(i - 1) {
                score += 5; // consecutive letters
            }
            if i == 0 || matches!(item[i - 1], '/' | '-' | '_' | '.') {
                score += 3; // start of a word
            }
            previous_match = Some(i);
            matched += 1;
        }
    }
    if matched == query.len() {
        Some(score * 4 - item.len() as i32) // shorter names win ties
    } else {
        None
    }
}

/// What a key press in the picker led to.
pub enum Outcome {
    Open,
    Cancelled,
    Picked(Vec<String>),
}

pub struct Picker {
    title: String,
    /// Keys for its bottom border, as `ui::key_hints` shows them.
    hints: Vec<(&'static str, &'static str)>,
    items: Vec<String>,
    query: String,
    /// Indexes into `items` that match the query, best first.
    matches: Vec<usize>,
    /// Over `matches`.
    cursor: ui::Cursor,
    /// Multi-select mode: space ticks names, which have no spaces, so it
    /// does while typing too. Enter returns the ticked ones.
    multi: bool,
    ticked: Vec<String>,
    /// Lets Enter return the typed text when nothing matches, e.g. a sha.
    free_text: bool,
    /// Keys go to the query rather than navigating, like vim's insert mode.
    /// A popup picker starts so, to type at once; `esc` leaves it for the
    /// letter keys, and `i` goes back.
    typing: bool,
    /// Items are paths: the folders leading to the last name are dimmed.
    dim_folders: bool,
    /// When set, the items listed instead of the query's fuzzy matches, e.g.
    /// the branches holding a commit whose sha was searched.
    only: Option<Vec<String>>,
}

impl Picker {
    pub fn single(title: &str, items: Vec<String>) -> Picker {
        let matches = (0..items.len()).collect();
        Picker {
            title: format!(" {title} "),
            hints: Vec::new(),
            items,
            query: String::new(),
            matches,
            cursor: ui::Cursor::default(),
            multi: false,
            ticked: Vec::new(),
            free_text: false,
            typing: false,
            dim_folders: false,
            only: None,
        }
    }

    pub fn multi(title: &str, items: Vec<String>, ticked: Vec<String>) -> Picker {
        Picker {
            multi: true,
            ticked,
            ..Picker::single(title, items)
        }
    }

    pub fn with_free_text(title: &str, items: Vec<String>) -> Picker {
        Picker {
            free_text: true,
            ..Picker::single(title, items)
        }
    }

    pub fn with_hints(self, hints: &[(&'static str, &'static str)]) -> Picker {
        Picker {
            hints: hints.to_vec(),
            ..self
        }
    }

    /// Ready to type in as it opens, as a popup picker is.
    pub fn typing(self) -> Picker {
        Picker {
            typing: true,
            ..self
        }
    }

    /// Shows items as paths, the last name in full and the folders before it dimmed.
    pub fn with_dimmed_folders(self) -> Picker {
        Picker {
            dim_folders: true,
            ..self
        }
    }

    pub fn accepts_free_text(&self) -> bool {
        self.free_text
    }

    /// Fills in the search from outside, e.g. from the clipboard, typing or
    /// not as before.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
        self.update_matches();
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Lists just these items, in the list's order, whatever the query, or
    /// with None goes back to matching the query.
    pub fn set_only(&mut self, only: Option<Vec<String>>) {
        if only != self.only {
            self.only = only;
            self.update_matches();
        }
    }

    pub fn is_typing(&self) -> bool {
        self.typing
    }

    pub fn stop_typing(&mut self) {
        self.typing = false;
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.update_matches();
    }

    pub fn is_searching(&self) -> bool {
        self.typing || !self.query.is_empty()
    }

    /// The items passing the filter, in display order.
    pub fn matched_items(&self) -> Vec<&str> {
        self.matches
            .iter()
            .map(|&i| self.items[i].as_str())
            .collect()
    }

    /// The position of the cursor in `matched_items`.
    pub fn selected_row(&self) -> usize {
        self.cursor.index
    }

    /// Replaces the list, keeping the cursor on the same item if it's still there.
    pub fn set_items(&mut self, items: Vec<String>) {
        let highlighted = self.highlighted().cloned();
        self.items = items;
        self.update_matches();
        if let Some(item) = highlighted {
            self.select_item(&item);
        }
    }

    /// Moves the cursor to `item`, if it's in the filtered list.
    pub fn select_item(&mut self, item: &str) {
        let position = self.matches.iter().position(|&i| self.items[i] == item);
        if let Some(position) = position {
            self.cursor.index = position;
        }
    }

    fn update_matches(&mut self) {
        self.cursor = ui::Cursor::default();
        if let Some(only) = &self.only {
            self.matches = (0..self.items.len())
                .filter(|&i| only.contains(&self.items[i]))
                .collect();
            return;
        }
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((fuzzy_score(&self.query, item)?, i)))
            .collect();
        // Stable sort, so equal scores keep the original order.
        scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
        self.matches = scored.into_iter().map(|(_, i)| i).collect();
    }

    pub fn highlighted(&self) -> Option<&String> {
        self.items.get(*self.matches.get(self.cursor.index)?)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return self.submit(),
            KeyCode::Down => self.move_down(),
            KeyCode::Up => self.move_up(),
            KeyCode::Char('j' | 'n') if ctrl => self.move_down(),
            KeyCode::Char('k' | 'p') if ctrl => self.move_up(),
            KeyCode::Char(' ') if self.multi => self.toggle_tick(),
            KeyCode::Char('a') if self.multi && ctrl => self.tick_all_matches(),
            KeyCode::Char('d') if self.multi && ctrl => self.untick_all_matches(),

            // Typing mode, where a popup picker starts. With nothing typed,
            // esc closes it, as there's nothing to keep.
            KeyCode::Esc if self.typing && self.query.is_empty() => return Outcome::Cancelled,
            KeyCode::Esc if self.typing => self.typing = false,
            KeyCode::Backspace if self.typing => {
                self.query.pop();
                self.update_matches();
            }
            KeyCode::Char(ch) if self.typing && !ctrl => {
                self.query.push(ch);
                self.update_matches();
            }

            // Navigation mode, after esc, or the Branches pane's search.
            KeyCode::Esc => return Outcome::Cancelled,
            KeyCode::Char('i') => self.typing = true,
            KeyCode::Char('a') if self.multi => self.tick_all_matches(),
            KeyCode::Char('A') if self.multi => self.untick_all_matches(),
            KeyCode::Char('j' | 'k' | 'g' | 'G') => {
                self.cursor.handle_key(key, self.matches.len());
            }
            _ => {}
        }
        Outcome::Open
    }

    fn move_down(&mut self) {
        self.cursor.down(self.matches.len());
    }

    fn move_up(&mut self) {
        self.cursor.up();
    }

    fn submit(&self) -> Outcome {
        if self.multi {
            return Outcome::Picked(self.ticked.clone());
        }
        match self.highlighted() {
            Some(item) => Outcome::Picked(vec![item.clone()]),
            None if self.free_text && !self.query.is_empty() => {
                Outcome::Picked(vec![self.query.clone()])
            }
            None => Outcome::Open,
        }
    }

    fn toggle_tick(&mut self) {
        let Some(item) = self.highlighted().cloned() else {
            return;
        };
        if let Some(position) = self.ticked.iter().position(|t| *t == item) {
            self.ticked.remove(position);
        } else {
            self.ticked.push(item);
        }
    }

    /// Ticks everything passing the filter, which without a search is everything.
    fn tick_all_matches(&mut self) {
        for &index in &self.matches {
            let item = &self.items[index];
            if !self.ticked.contains(item) {
                self.ticked.push(item.clone());
            }
        }
    }

    fn untick_all_matches(&mut self) {
        let shown: Vec<&String> = self
            .matches
            .iter()
            .map(|&index| &self.items[index])
            .collect();
        self.ticked.retain(|item| !shown.contains(&item));
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let popup = ui::centered(area, 60, 20);
        frame.render_widget(Clear, popup);
        let block = ui::panel(self.title.clone(), true)
            .title_bottom(ui::key_hints(&self.shown_hints(), None));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.height == 0 {
            return;
        }
        self.render_query(frame, Rect { height: 1, ..inner });

        let list = Rect {
            y: inner.y + 1,
            height: inner.height - 1,
            ..inner
        };
        let scroll = self
            .cursor
            .scroll_to(self.cursor.index, list.height as usize);
        let lines: Vec<Line> = self
            .matches
            .iter()
            .enumerate()
            .skip(scroll)
            .map(|(row, &item)| {
                self.item_line(
                    &self.items[item],
                    row == self.cursor.index,
                    list.width as usize,
                )
            })
            .collect();
        if lines.is_empty() {
            let note = if self.free_text {
                "No match. Enter uses the typed text as a ref"
            } else {
                "No match"
            };
            frame.render_widget(Line::styled(note, Style::default().fg(ui::DIM)), list);
        } else {
            ui::render_lines(frame, list, lines);
        }
    }

    /// The hints for the border. `i` only shows with a search in the box and
    /// typing stopped: an empty box says "i to search" itself, and while
    /// typing, i is a letter. Nor does `esc` while typing, as the search box
    /// says what it does then.
    fn shown_hints(&self) -> Vec<(&'static str, &'static str)> {
        let needs_i = !self.typing && !self.query.is_empty();
        let shown = |key: &str| match key {
            "i" => needs_i,
            "esc" => !self.typing,
            _ => true,
        };
        self.hints
            .iter()
            .copied()
            .filter(|(key, _)| shown(key))
            .collect()
    }

    pub fn render_query(&self, frame: &mut Frame, area: Rect) {
        let mut query = vec![
            Span::styled("> ", Style::default().fg(ui::accent())),
            Span::raw(self.query.as_str()),
        ];
        // Each mode says how to leave it, right where the search is: "i to
        // search" before typing, and "esc done" just past the cursor while
        // typing, moving along as the search grows.
        if self.typing {
            let esc = if self.query.is_empty() {
                " esc close"
            } else {
                " esc done"
            };
            query.push(Span::styled("▏", Style::default().fg(ui::accent())));
            query.push(Span::styled(esc, Style::default().fg(ui::DIM)));
        } else if self.query.is_empty() {
            query.push(Span::styled("i to search", Style::default().fg(ui::DIM)));
        }
        frame.render_widget(Line::from(query), area);
    }

    /// An item's row. A path's folders give way, so its file's name shows.
    fn item_line<'a>(&self, item: &'a str, selected: bool, width: usize) -> Line<'a> {
        let mut spans = Vec::new();
        if self.multi {
            let mark = if self.ticked.iter().any(|t| t == item) {
                "◉ "
            } else {
                "○ "
            };
            spans.push(Span::styled(mark, Style::default().fg(Color::Green)));
        }
        let mut shrinkable = Vec::new();
        match item.rfind('/').filter(|_| self.dim_folders) {
            Some(slash) => {
                let (folders, name) = item.split_at(slash + 1);
                shrinkable.push((spans.len(), 1));
                spans.push(Span::styled(folders, Style::default().fg(ui::DIM)));
                spans.push(Span::raw(name));
            }
            None => spans.push(Span::raw(item)),
        }
        let line = ui::fit(Line::from(spans), &shrinkable, width);
        // A picker is focused whenever it shows.
        ui::selected(line, selected, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(picker: &mut Picker, text: &str) {
        picker.handle_key(key(KeyCode::Char('i')));
        for ch in text.chars() {
            picker.handle_key(key(KeyCode::Char(ch)));
        }
    }

    #[test]
    fn the_i_hint_only_shows_when_the_search_box_doesnt_say_it() {
        let hints = [("i", "type"), ("esc", "close")];
        let mut picker = Picker::single("t", vec!["a".to_string()]).with_hints(&hints);
        assert_eq!(picker.shown_hints(), [("esc", "close")]); // "i to search" in the box
        picker.set_query("a".to_string());
        assert_eq!(picker.shown_hints(), hints);
        picker.handle_key(KeyEvent::from(KeyCode::Char('i')));
        assert!(picker.shown_hints().is_empty()); // typing: "esc done" is in the box
    }

    #[test]
    fn dimmed_folders_leave_the_last_name_in_full() {
        let picker = Picker::single("t", Vec::new()).with_dimmed_folders();
        let line = picker.item_line("group/sub/repo", false, 200);
        let parts: Vec<(&str, Option<Color>)> = line
            .spans
            .iter()
            .map(|span| (span.content.as_ref(), span.style.fg))
            .collect();
        assert_eq!(parts, [("group/sub/", Some(ui::DIM)), ("repo", None)]);

        let plain = Picker::single("t", Vec::new()).item_line("feature/bar", false, 200);
        assert_eq!(plain.spans.len(), 1);
    }

    #[test]
    fn query_must_be_a_subsequence() {
        assert!(fuzzy_score("fbr", "feature/bar").is_some());
        assert!(fuzzy_score("rbf", "feature/bar").is_none());
        assert!(fuzzy_score("ZZ", "feature").is_none());
    }

    #[test]
    fn matching_ignores_case() {
        assert!(fuzzy_score("MAIN", "main").is_some());
    }

    #[test]
    fn consecutive_and_word_start_matches_rank_higher() {
        let tight = fuzzy_score("bar", "feature/bar").unwrap();
        let scattered = fuzzy_score("bar", "bxaxrxxxxxx").unwrap();
        assert!(tight > scattered);
    }

    #[test]
    fn clearing_the_query_restores_the_original_order() {
        let mut picker = Picker::single("t", vec!["longer-name".to_string(), "a".to_string()]);
        type_text(&mut picker, "a");
        picker.handle_key(key(KeyCode::Backspace));
        assert_eq!(picker.matches, [0, 1]);
    }

    #[test]
    fn typing_filters_and_enter_picks_the_best_match() {
        let items = vec!["main".to_string(), "feature/parser".to_string()];
        let mut picker = Picker::single("t", items);
        type_text(&mut picker, "pars");
        match picker.handle_key(key(KeyCode::Enter)) {
            Outcome::Picked(picked) => assert_eq!(picked, ["feature/parser"]),
            _ => panic!("expected a pick"),
        }
    }

    #[test]
    fn letters_navigate_until_i_starts_typing() {
        let items = vec!["a".to_string(), "b".to_string(), "jk".to_string()];
        let mut picker = Picker::single("t", items);
        picker.handle_key(key(KeyCode::Char('j')));
        assert_eq!((picker.cursor.index, picker.query.as_str()), (1, ""));

        type_text(&mut picker, "jk");
        assert_eq!(picker.query, "jk");
        assert_eq!(picker.matches, [2]);
    }

    #[test]
    fn esc_stops_typing_first_and_cancels_second() {
        let mut picker = Picker::single("t", vec!["main".to_string()]);
        type_text(&mut picker, "m");
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            Outcome::Open
        ));
        assert!(!picker.typing);
        assert_eq!(picker.query, "m");
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            Outcome::Cancelled
        ));
    }

    fn picked(picker: &mut Picker) -> Vec<String> {
        match picker.handle_key(key(KeyCode::Enter)) {
            Outcome::Picked(picked) => picked,
            _ => panic!("expected a pick"),
        }
    }

    #[test]
    fn a_ticks_everything_and_capital_a_unticks_everything() {
        let items = vec!["x".to_string(), "y".to_string()];
        let mut picker = Picker::multi("t", items, Vec::new());
        picker.handle_key(key(KeyCode::Char('a')));
        assert_eq!(picked(&mut picker), ["x", "y"]);
        picker.handle_key(key(KeyCode::Char('A')));
        assert!(picked(&mut picker).is_empty());
    }

    #[test]
    fn a_keeps_earlier_ticks_first_without_repeating_them() {
        let items = vec!["x".to_string(), "y".to_string()];
        let mut picker = Picker::multi("t", items, vec!["y".to_string()]);
        picker.handle_key(key(KeyCode::Char('a')));
        assert_eq!(picked(&mut picker), ["y", "x"]);
    }

    #[test]
    fn a_and_capital_a_only_touch_the_filtered_items() {
        let items = vec!["origin/x".to_string(), "main".to_string()];
        let mut picker = Picker::multi("t", items, vec!["main".to_string()]);
        type_text(&mut picker, "origin");
        picker.handle_key(key(KeyCode::Esc));
        picker.handle_key(key(KeyCode::Char('a')));
        assert_eq!(picked(&mut picker), ["main", "origin/x"]);
        picker.handle_key(key(KeyCode::Char('A')));
        assert_eq!(picked(&mut picker), ["main"]);
    }

    #[test]
    fn a_is_a_letter_while_typing() {
        let items = vec!["x".to_string(), "a".to_string()];
        let mut picker = Picker::multi("t", items, Vec::new());
        type_text(&mut picker, "a");
        assert_eq!(picker.query, "a");
        assert!(picker.ticked.is_empty());
    }

    #[test]
    fn a_does_nothing_in_a_single_choice_picker() {
        let mut picker = Picker::single("t", vec!["x".to_string()]);
        picker.handle_key(key(KeyCode::Char('a')));
        assert!(picker.ticked.is_empty());
    }

    #[test]
    fn moving_stops_at_the_ends_of_the_filtered_list() {
        let mut picker = Picker::single("t", vec!["a".to_string(), "b".to_string()]);
        picker.handle_key(key(KeyCode::Char('k')));
        assert_eq!(picker.highlighted().unwrap(), "a");
        picker.handle_key(key(KeyCode::Char('j')));
        picker.handle_key(key(KeyCode::Char('j')));
        assert_eq!(picker.highlighted().unwrap(), "b");
    }

    #[test]
    fn set_items_keeps_the_cursor_on_the_same_item() {
        let mut picker = Picker::single("t", vec!["a".to_string(), "b".to_string()]);
        picker.handle_key(key(KeyCode::Char('j')));
        picker.set_items(vec!["new".to_string(), "a".to_string(), "b".to_string()]);
        assert_eq!(picker.highlighted().unwrap(), "b");
    }

    #[test]
    fn select_item_ignores_names_that_are_filtered_out() {
        let items = vec!["main".to_string(), "feature".to_string()];
        let mut picker = Picker::single("t", items);
        type_text(&mut picker, "fea");
        picker.select_item("main");
        assert_eq!(picker.highlighted().unwrap(), "feature");
    }

    #[test]
    fn clearing_the_query_lists_everything_again() {
        let items = vec!["main".to_string(), "feature".to_string()];
        let mut picker = Picker::single("t", items);
        type_text(&mut picker, "fea");
        picker.clear_query();
        assert_eq!(picker.matches, [0, 1]);
    }

    #[test]
    fn free_text_is_only_used_when_allowed() {
        let mut strict = Picker::single("t", vec!["main".to_string()]);
        type_text(&mut strict, "xyz123");
        assert!(matches!(
            strict.handle_key(key(KeyCode::Enter)),
            Outcome::Open
        ));

        let mut free = Picker::with_free_text("t", vec!["main".to_string()]);
        type_text(&mut free, "xyz123");
        match free.handle_key(key(KeyCode::Enter)) {
            Outcome::Picked(picked) => assert_eq!(picked, ["xyz123"]),
            _ => panic!("expected the typed text"),
        }
    }

    #[test]
    fn set_query_filters_and_keeps_typing_or_not() {
        let items = vec!["main".to_string(), "other".to_string()];
        let mut picker = Picker::with_free_text("t", items);
        type_text(&mut picker, "x");
        picker.set_query("main".to_string());
        assert_eq!(picker.query, "main");
        assert!(picker.typing);
        assert_eq!(picker.matches, [0]);
        picker.stop_typing();
        picker.set_query("other".to_string());
        assert!(!picker.typing);
    }

    #[test]
    fn a_typing_picker_closes_on_esc_with_nothing_typed_and_stops_typing_with_a_search() {
        let items = vec!["main".to_string(), "other".to_string()];
        let mut picker = Picker::single("t", items.clone()).typing();
        // Letters search at once.
        picker.handle_key(key(KeyCode::Char('o')));
        assert_eq!(picker.matched_items(), ["other"]);
        // esc keeps the search, for the letter keys, and closes after.
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            Outcome::Open
        ));
        assert!(!picker.is_typing());
        assert!(matches!(
            picker.handle_key(key(KeyCode::Esc)),
            Outcome::Cancelled
        ));
        let mut empty = Picker::single("t", items).typing();
        assert!(matches!(
            empty.handle_key(key(KeyCode::Esc)),
            Outcome::Cancelled
        ));
    }

    #[test]
    fn ctrl_a_and_ctrl_d_tick_and_untick_what_a_search_shows_while_typing() {
        let items = vec![
            "main".to_string(),
            "feat/a".to_string(),
            "feat/b".to_string(),
        ];
        let mut picker = Picker::multi("t", items, vec!["main".to_string()]).typing();
        let ctrl = |ch| KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL);
        for ch in "feat".chars() {
            picker.handle_key(key(KeyCode::Char(ch)));
        }
        picker.handle_key(ctrl('a'));
        assert_eq!(picker.ticked, ["main", "feat/a", "feat/b"]);
        // space ticks while typing, as names have no spaces.
        picker.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(picker.query, "feat");
        assert_eq!(picker.ticked, ["main", "feat/b"]);
        picker.handle_key(ctrl('d'));
        assert_eq!(picker.ticked, ["main"]);
    }

    #[test]
    fn set_only_lists_those_items_whatever_the_query() {
        let items = vec![
            "main".to_string(),
            "feature".to_string(),
            "other".to_string(),
        ];
        let mut picker = Picker::single("t", items);
        type_text(&mut picker, "abc1234");
        assert!(picker.matches.is_empty());
        picker.set_only(Some(vec!["other".to_string(), "main".to_string()]));
        assert_eq!(picker.matched_items(), ["main", "other"]);
        picker.set_only(None);
        assert!(picker.matches.is_empty());
    }

    #[test]
    fn space_ticks_in_multi_mode() {
        let items = vec!["a".to_string(), "b".to_string()];
        let mut picker = Picker::multi("t", items, Vec::new());
        picker.handle_key(key(KeyCode::Char(' ')));
        picker.handle_key(key(KeyCode::Down));
        picker.handle_key(key(KeyCode::Char(' ')));
        match picker.handle_key(key(KeyCode::Enter)) {
            Outcome::Picked(picked) => assert_eq!(picked, ["a", "b"]),
            _ => panic!("expected a pick"),
        }
    }
}
