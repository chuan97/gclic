//! A commit's message and metadata, shown on the right like a diff.

use crate::git::{self, ChangeSize, CommitDetails};
use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub struct DetailsPane {
    details: CommitDetails,
    /// How big the commit is, which the worker counts, as it can take a while.
    size: Size,
    scroll: ui::Scroll,
}

enum Size {
    Counting,
    Counted(ChangeSize),
    /// Git couldn't count it.
    Unknown,
}

impl DetailsPane {
    /// The details, with the size still to count.
    pub fn new(details: CommitDetails) -> DetailsPane {
        DetailsPane {
            details,
            size: Size::Counting,
            scroll: ui::Scroll::default(),
        }
    }

    pub fn sha(&self) -> &str {
        &self.details.sha
    }

    /// The commit whose size is still to count, if it is.
    pub fn sha_to_count(&self) -> Option<&str> {
        matches!(self.size, Size::Counting).then_some(self.sha())
    }

    /// Its size, once counted, or None if git couldn't count it.
    pub fn set_size(&mut self, size: Option<ChangeSize>) {
        self.size = size.map_or(Size::Unknown, Size::Counted);
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.scroll.handle_key(key);
    }

    fn lines<'a>(details: &'a CommitDetails, size: &Size) -> Vec<Line<'a>> {
        let label = Style::default().fg(ui::DIM);
        let mut lines = vec![
            Line::from(vec![
                Span::styled("sha    ", label),
                Span::raw(details.sha.as_str()),
            ]),
            Line::from(vec![
                Span::styled("author ", label),
                Span::raw(details.author.as_str()),
            ]),
            Line::from(vec![
                Span::styled("date   ", label),
                Span::raw(details.date.as_str()),
            ]),
            Line::from(changes_spans(size)),
            Line::default(),
        ];
        // Terminals can't draw tabs in a cell, and the widget would drop them.
        lines.extend(
            details
                .message
                .lines()
                .map(|line| Line::raw(line.replace('\t', "    "))),
        );
        lines
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let lines = DetailsPane::lines(&self.details, &self.size);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        // Borders take a row or column on each side.
        let total = paragraph.line_count(area.width.saturating_sub(2));
        let page = area.height.saturating_sub(2) as usize;
        self.scroll.fit(page, total);
        let scroll = self.scroll.offset();

        let short_sha = git::short(&self.details.sha);
        let position = if total > page {
            format!(" ({}/{total})", scroll + page)
        } else {
            String::new()
        };
        let block = ui::panel(format!(" Commit {short_sha}{position} "), focused);
        frame.render_widget(paragraph.block(block).scroll((scroll as u16, 0)), area);
    }
}

/// `changes 3 files  +42 −7`: how big the commit is, in the graph's colours,
/// as there's no room for its files beside the panes in a sidebar.
fn changes_spans(size: &Size) -> Vec<Span<'static>> {
    let label = Style::default().fg(ui::DIM);
    let mut spans = vec![Span::styled("changes ", label)];
    let size = match size {
        Size::Counting => return vec![Span::styled("changes counting…", label)],
        Size::Unknown => return vec![Span::styled("changes unknown", label)],
        Size::Counted(size) => size,
    };
    if size.files == 0 {
        spans.push(Span::styled("none", label));
        return spans;
    }
    let noun = if size.files == 1 { "file" } else { "files" };
    spans.push(Span::raw(format!("{} {noun}", size.files)));
    let counts = ui::line_count_spans(size.lines);
    if !counts.is_empty() {
        spans.push(Span::raw("  "));
        spans.extend(counts);
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyCode;

    fn popup(message: &str) -> DetailsPane {
        DetailsPane::new(CommitDetails {
            sha: "abc1234def".to_string(),
            author: "Ann".to_string(),
            date: "today".to_string(),
            message: message.to_string(),
        })
    }

    fn shown(popup: &mut DetailsPane) -> String {
        let mut terminal = Terminal::new(TestBackend::new(40, 10)).unwrap();
        terminal
            .draw(|frame| popup.render(frame, frame.area(), true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        let cells: Vec<&str> = buffer.content().iter().map(|cell| cell.symbol()).collect();
        cells.chunks(width).map(|row| row.concat() + "\n").collect()
    }

    fn draw(popup: &mut DetailsPane, width: u16, height: u16) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| popup.render(frame, frame.area(), true))
            .unwrap();
    }

    fn press(popup: &mut DetailsPane, ch: char) {
        popup.handle_key(KeyEvent::from(KeyCode::Char(ch)));
    }

    #[test]
    fn scrolling_stops_at_both_ends() {
        // 5 lines of header plus 50 of message, in 12 rows of text.
        let mut popup = popup(&vec!["line"; 50].join("\n"));
        draw(&mut popup, 100, 14);
        press(&mut popup, 'k');
        assert_eq!(popup.scroll.offset(), 0);

        press(&mut popup, 'G');
        draw(&mut popup, 100, 14);
        press(&mut popup, 'j');
        draw(&mut popup, 100, 14);
        assert_eq!(popup.scroll.offset(), 55 - 12);
    }

    #[test]
    fn the_header_says_how_many_files_and_lines_the_commit_changes_once_counted() {
        let mut popup = popup("Fix it");
        assert_eq!(popup.sha_to_count(), Some("abc1234def"));
        let counting = shown(&mut popup);
        assert!(counting.contains("changes counting…"), "{counting}");
        popup.set_size(Some(ChangeSize {
            files: 2,
            lines: git::Lines::Counted {
                added: 12,
                removed: 3,
            },
        }));
        assert_eq!(popup.sha_to_count(), None);
        let counted = shown(&mut popup);
        assert!(counted.contains("changes 2 files  +12 −3"), "{counted}");
        popup.set_size(None);
        let unknown = shown(&mut popup);
        assert!(unknown.contains("changes unknown"), "{unknown}");
    }

    #[test]
    fn wrapped_lines_count_towards_the_end() {
        // A 42-column pane leaves 40 for text, so this one line takes many rows.
        let mut popup = popup(&"word ".repeat(150));
        draw(&mut popup, 42, 14);
        press(&mut popup, 'G');
        draw(&mut popup, 42, 14);
        assert!(
            popup.scroll.offset() > 4,
            "scroll was {}",
            popup.scroll.offset()
        );
    }

    #[test]
    fn a_short_message_does_not_scroll() {
        let mut popup = popup("short");
        draw(&mut popup, 100, 30);
        press(&mut popup, 'j');
        draw(&mut popup, 100, 30);
        assert_eq!(popup.scroll.offset(), 0);
    }

    #[test]
    fn a_tiny_terminal_does_not_panic() {
        let mut popup = popup(&vec!["line"; 50].join("\n"));
        draw(&mut popup, 3, 2);
        press(&mut popup, 'G');
        draw(&mut popup, 3, 2);
    }
}
