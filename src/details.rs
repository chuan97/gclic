//! A commit's message and metadata, shown on the right like a diff.

use crate::git::CommitDetails;
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub struct DetailsPane {
    details: CommitDetails,
    scroll: usize,
    /// Rows of text on screen, known once drawn.
    page: usize,
    /// The furthest it can scroll, also known once drawn.
    max_scroll: usize,
}

impl DetailsPane {
    pub fn new(details: CommitDetails) -> DetailsPane {
        DetailsPane { details, scroll: 0, page: 0, max_scroll: 0 }
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
                self.scroll = (self.scroll + 1).min(self.max_scroll);
            }
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
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

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let paragraph = Paragraph::new(self.lines()).wrap(Wrap { trim: false });
        // Borders take a row or column on each side.
        let total = paragraph.line_count(area.width.saturating_sub(2));
        let page = area.height.saturating_sub(2) as usize;
        let max_scroll = total.saturating_sub(page);
        let scroll = self.scroll.min(max_scroll);

        let short_sha = &self.details.sha[..self.details.sha.len().min(7)];
        let position = if total > page {
            format!(" ({}/{total})", scroll + page)
        } else {
            String::new()
        };
        let block = ui::panel(format!(" Commit {short_sha}{position} "), focused);
        frame.render_widget(paragraph.block(block).scroll((scroll as u16, 0)), area);

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

    fn popup(message: &str) -> DetailsPane {
        DetailsPane::new(CommitDetails {
            sha: "abc1234def".to_string(),
            author: "Ann".to_string(),
            date: "today".to_string(),
            message: message.to_string(),
        })
    }

    fn draw(popup: &mut DetailsPane, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| popup.render(frame, frame.area(), true)).unwrap();
    }

    fn press(popup: &mut DetailsPane, ch: char) {
        popup.handle_key(KeyEvent::from(KeyCode::Char(ch)));
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        // 4 lines of header plus 50 of message, in 12 rows of text.
        let mut popup = popup(&vec!["line"; 50].join("\n"));
        draw(&mut popup, 100, 14);
        press(&mut popup, 'k');
        assert_eq!(popup.scroll, 0);

        press(&mut popup, 'G');
        draw(&mut popup, 100, 14);
        press(&mut popup, 'j');
        draw(&mut popup, 100, 14);
        assert_eq!(popup.scroll, 54 - 12);
    }

    #[test]
    fn wrapped_lines_count_towards_the_end() {
        // A 42-column pane leaves 40 for text, so this one line takes many rows.
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
