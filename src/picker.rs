//! A fuzzy-search popup over a list of names, used for checkout, for choosing
//! graph branches and for choosing refs to compare.

use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
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
    items: Vec<String>,
    query: String,
    /// Indexes into `items` that match the query, best first.
    matches: Vec<usize>,
    selected: usize,
    scroll: usize,
    /// Multi-select mode: Space ticks names, Enter returns the ticked ones.
    multi: bool,
    ticked: Vec<String>,
    /// Lets Enter return the typed text when nothing matches, e.g. a sha.
    free_text: bool,
    /// Keys go to the query rather than navigating, like vim's insert mode.
    typing: bool,
}

impl Picker {
    pub fn single(title: &str, items: Vec<String>) -> Picker {
        let matches = (0..items.len()).collect();
        Picker {
            title: format!(" {title} "),
            items,
            query: String::new(),
            matches,
            selected: 0,
            scroll: 0,
            multi: false,
            ticked: Vec::new(),
            free_text: false,
            typing: false,
        }
    }

    pub fn multi(title: &str, items: Vec<String>, ticked: Vec<String>) -> Picker {
        Picker { multi: true, ticked, ..Picker::single(title, items) }
    }

    pub fn with_free_text(title: &str, items: Vec<String>) -> Picker {
        Picker { free_text: true, ..Picker::single(title, items) }
    }

    fn update_matches(&mut self) {
        let mut scored: Vec<(i32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((fuzzy_score(&self.query, item)?, i)))
            .collect();
        // Stable sort, so equal scores keep the original order.
        scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
        self.matches = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
        self.scroll = 0;
    }

    fn highlighted(&self) -> Option<&String> {
        self.items.get(*self.matches.get(self.selected)?)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = self.matches.len().saturating_sub(1);
        match key.code {
            KeyCode::Enter => return self.submit(),
            KeyCode::Down => self.selected = (self.selected + 1).min(last),
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('j' | 'n') if ctrl => self.selected = (self.selected + 1).min(last),
            KeyCode::Char('k' | 'p') if ctrl => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char(' ') if self.multi => self.toggle_tick(),

            // Typing mode, entered with `i`.
            KeyCode::Esc if self.typing => self.typing = false,
            KeyCode::Backspace if self.typing => {
                self.query.pop();
                self.update_matches();
            }
            KeyCode::Char(ch) if self.typing && !ctrl => {
                self.query.push(ch);
                self.update_matches();
            }

            // Navigation mode, the default.
            KeyCode::Esc => return Outcome::Cancelled,
            KeyCode::Char('i') => self.typing = true,
            KeyCode::Char('j') => self.selected = (self.selected + 1).min(last),
            KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('g') => self.selected = 0,
            KeyCode::Char('G') => self.selected = last,
            _ => {}
        }
        Outcome::Open
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

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let popup = ui::centered(area, 60, 20);
        frame.render_widget(Clear, popup);
        let block = ui::panel(self.title.clone(), true);
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        if inner.height < 2 {
            return;
        }

        let mut query = vec![
            Span::styled("> ", Style::default().fg(Color::Cyan)),
            Span::raw(self.query.as_str()),
        ];
        if self.typing {
            query.push(Span::styled("▏", Style::default().fg(Color::Cyan)));
        } else if self.query.is_empty() {
            query.push(Span::styled("i to search", Style::default().fg(ui::DIM)));
        }
        frame.render_widget(Line::from(query), Rect { height: 1, ..inner });

        let list = Rect { y: inner.y + 1, height: inner.height - 1, ..inner };
        self.scroll = ui::keep_visible(self.scroll, self.selected, list.height as usize);
        let lines: Vec<Line> = self
            .matches
            .iter()
            .enumerate()
            .skip(self.scroll)
            .map(|(row, &item)| self.item_line(&self.items[item], row == self.selected))
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

    fn item_line<'a>(&self, item: &'a str, selected: bool) -> Line<'a> {
        let mut spans = Vec::new();
        if self.multi {
            let mark = if self.ticked.iter().any(|t| t == item) { "◉ " } else { "○ " };
            spans.push(Span::styled(mark, Style::default().fg(Color::Green)));
        }
        spans.push(Span::raw(item));
        let line = Line::from(spans);
        if selected {
            line.style(Style::default().bg(ui::SELECTED_BG).add_modifier(Modifier::BOLD))
        } else {
            line
        }
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
        assert_eq!((picker.selected, picker.query.as_str()), (1, ""));

        type_text(&mut picker, "jk");
        assert_eq!(picker.query, "jk");
        assert_eq!(picker.matches, [2]);
    }

    #[test]
    fn esc_stops_typing_first_and_cancels_second() {
        let mut picker = Picker::single("t", vec!["main".to_string()]);
        type_text(&mut picker, "m");
        assert!(matches!(picker.handle_key(key(KeyCode::Esc)), Outcome::Open));
        assert!(!picker.typing);
        assert_eq!(picker.query, "m");
        assert!(matches!(picker.handle_key(key(KeyCode::Esc)), Outcome::Cancelled));
    }

    #[test]
    fn free_text_is_only_used_when_allowed() {
        let mut strict = Picker::single("t", vec!["main".to_string()]);
        type_text(&mut strict, "xyz123");
        assert!(matches!(strict.handle_key(key(KeyCode::Enter)), Outcome::Open));

        let mut free = Picker::with_free_text("t", vec!["main".to_string()]);
        type_text(&mut free, "xyz123");
        match free.handle_key(key(KeyCode::Enter)) {
            Outcome::Picked(picked) => assert_eq!(picked, ["xyz123"]),
            _ => panic!("expected the typed text"),
        }
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
