//! The popup showing a commit's message and metadata.

use crate::git::CommitDetails;
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

const WIDTH: u16 = 80;

pub struct DetailsPopup {
    details: CommitDetails,
    scroll: usize,
    /// Rows of text on screen, known once drawn.
    page: usize,
    /// The furthest it can scroll, also known once drawn.
    max_scroll: usize,
}

impl DetailsPopup {
    pub fn new(details: CommitDetails) -> DetailsPopup {
        DetailsPopup { details, scroll: 0, page: 0, max_scroll: 0 }
    }

    pub fn sha(&self) -> &str {
        &self.details.sha
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        let half_page = (self.page / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('d') if ctrl => {
                self.scroll = (self.scroll + half_page).min(self.max_scroll);
            }
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(half_page),
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll = if self.scroll >= self.max_scroll { 0 } else { self.scroll + 1 };
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.scroll = if self.scroll == 0 { self.max_scroll } else { self.scroll - 1 };
            }
            KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('G') => self.scroll = self.max_scroll,
            _ => {}
        }
    }

    fn lines(&self) -> Vec<Line<'_>> {
        let label = Style::default().fg(ui::DIM);
        let details = &self.details;
        let mut lines = vec![
            Line::from(vec![Span::styled("sha    ", label), Span::raw(details.sha.as_str())]),
            Line::from(vec![Span::styled("author ", label), Span::raw(details.author.as_str())]),
            Line::from(vec![Span::styled("date   ", label), Span::raw(details.date.as_str())]),
            Line::default(),
        ];
        // Terminals can't draw tabs in a cell, and the widget would drop them.
        lines.extend(details.message.lines().map(|line| Line::raw(line.replace('\t', "    "))));
        lines
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let width = WIDTH.min(area.width.saturating_sub(4));
        let paragraph = Paragraph::new(self.lines()).wrap(Wrap { trim: false });
        let total = paragraph.line_count(width.saturating_sub(2));

        // As tall as the text needs, up to nearly the whole screen.
        let height = (total + 2).min(area.height.saturating_sub(2) as usize);
        let page = height.saturating_sub(2);
        let max_scroll = total.saturating_sub(page);
        let scroll = self.scroll.min(max_scroll);

        let short_sha = &self.details.sha[..self.details.sha.len().min(7)];
        let position = if total > page {
            format!(" ({}/{total})", scroll + page)
        } else {
            String::new()
        };
        let block = ui::panel(format!(" Commit {short_sha}{position} "), true)
            .title_bottom(" j/k scroll · y copy sha · esc close ");

        let popup = ui::centered(area, width, height as u16);
        frame.render_widget(Clear, popup);
        frame.render_widget(paragraph.block(block).scroll((scroll as u16, 0)), popup);

        self.page = page;
        self.max_scroll = max_scroll;
        self.scroll = scroll;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn popup(message: &str) -> DetailsPopup {
        DetailsPopup::new(CommitDetails {
            sha: "abc1234def".to_string(),
            author: "Ann".to_string(),
            date: "today".to_string(),
            message: message.to_string(),
        })
    }

    fn draw(popup: &mut DetailsPopup, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| popup.render(frame, frame.area())).unwrap();
    }

    fn press(popup: &mut DetailsPopup, ch: char) {
        popup.handle_key(KeyEvent::from(KeyCode::Char(ch)));
    }

    #[test]
    fn scrolling_wraps_around_both_ends() {
        // 4 lines of header plus 50 of message, in 10 rows of text.
        let mut popup = popup(&vec!["line"; 50].join("\n"));
        draw(&mut popup, 100, 14);
        press(&mut popup, 'G');
        draw(&mut popup, 100, 14);
        assert_eq!(popup.scroll, 54 - 10);

        press(&mut popup, 'j');
        draw(&mut popup, 100, 14);
        assert_eq!(popup.scroll, 0);

        press(&mut popup, 'k');
        draw(&mut popup, 100, 14);
        assert_eq!(popup.scroll, 54 - 10);
    }

    #[test]
    fn wrapped_lines_count_towards_the_end() {
        // A 42-column screen leaves 36 for text, so this one line takes many rows.
        let mut popup = popup(&"word ".repeat(150));
        draw(&mut popup, 42, 14);
        press(&mut popup, 'G');
        draw(&mut popup, 42, 14);
        assert!(popup.scroll > 4, "scroll was {}", popup.scroll);
    }

    #[test]
    fn a_short_message_does_not_scroll() {
        let mut popup = popup("short");
        draw(&mut popup, 100, 30);
        press(&mut popup, 'j');
        draw(&mut popup, 100, 30);
        assert_eq!(popup.scroll, 0);
    }

    #[test]
    fn a_tiny_terminal_does_not_panic() {
        let mut popup = popup(&vec!["line"; 50].join("\n"));
        draw(&mut popup, 3, 2);
        press(&mut popup, 'G');
        draw(&mut popup, 3, 2);
    }
}
