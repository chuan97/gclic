//! Small drawing helpers shared by the panels.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

pub const SELECTED_BG: Color = Color::Indexed(237);
pub const DIM: Color = Color::DarkGray;

pub fn panel(title: String, focused: bool) -> Block<'static> {
    let color = if focused { Color::Cyan } else { DIM };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(title)
}

/// The scroll offset that keeps `selected` on screen, moving as little as possible.
pub fn keep_visible(scroll: usize, selected: usize, height: usize) -> usize {
    if selected < scroll {
        selected
    } else if selected >= scroll + height {
        selected + 1 - height
    } else {
        scroll
    }
}

/// Draws `lines` one per row. Unlike a Paragraph, a line's background style
/// fills its whole row, which is what makes a selection bar span the panel.
pub fn render_lines(frame: &mut Frame, area: Rect, lines: Vec<Line>) {
    for (i, line) in lines.into_iter().take(area.height as usize).enumerate() {
        let row = Rect { y: area.y + i as u16, height: 1, ..area };
        frame.render_widget(line, row);
    }
}

pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

pub fn status_color(status: char) -> Color {
    match status {
        'A' | '?' => Color::Green,
        'D' => Color::Red,
        'U' => Color::Magenta,
        _ => Color::Yellow,
    }
}
