//! Small drawing helpers shared by the panels.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

pub const SELECTED_BG: Color = Color::Indexed(237);
pub const DIM: Color = Color::DarkGray;
/// The colour of keys wherever they're listed: popups and the help.
pub const KEY: Color = Color::Yellow;

/// Keys and what they do, for a popup's bottom border: keys in `KEY` and
/// actions in the normal colour. The key for the action named `danger`, if
/// any, is bold red instead, as it's the key that's about to be pressed.
pub fn key_hints(hints: &[(&str, &str)], danger: Option<&str>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, action)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(DIM)));
        }
        let key_style = if danger == Some(*action) {
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(KEY)
        };
        spans.push(Span::styled(key.to_string(), key_style));
        // Reset rather than left unset, as a border's colour would show through.
        spans.push(Span::styled(format!(" {action}"), Style::default().fg(Color::Reset)));
    }
    spans.push(Span::raw(" "));
    Line::from(spans).right_aligned()
}

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

/// The next position in a list of `len` items, wrapping past the end to the start.
pub fn next_index(index: usize, len: usize) -> usize {
    if index + 1 >= len { 0 } else { index + 1 }
}

/// The previous position in a list of `len` items, wrapping past the start to the end.
pub fn previous_index(index: usize, len: usize) -> usize {
    if index == 0 { len.saturating_sub(1) } else { index - 1 }
}

/// The positions one key away in a list of `len`, most likely first: the
/// selected one, then where j, k, g and G lead. Used to prefetch diffs.
pub fn one_key_away(selected: usize, len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let mut positions = Vec::new();
    let candidates = [selected, next_index(selected, len), previous_index(selected, len), 0, len - 1];
    for position in candidates {
        if !positions.contains(&position) {
            positions.push(position);
        }
    }
    positions
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_wraps_around_both_ends() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(2, 3), 0);
        assert_eq!(previous_index(1, 3), 0);
        assert_eq!(previous_index(0, 3), 2);
    }

    #[test]
    fn key_hints_colour_keys_and_the_dangerous_action() {
        let line = key_hints(&[("enter", "discard"), ("esc", "cancel")], Some("discard"));
        let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text, " enter discard · esc cancel ");
        let style_of = |content: &str| line.spans.iter().find(|s| s.content == content).unwrap().style;
        assert_eq!(style_of("enter").fg, Some(Color::Red));
        assert_eq!(style_of("esc").fg, Some(KEY));
        assert_eq!(style_of(" discard").fg, Some(Color::Reset));
        assert_eq!(style_of(" cancel").fg, Some(Color::Reset));
    }

    #[test]
    fn one_key_away_lists_each_position_once_most_likely_first() {
        assert_eq!(one_key_away(2, 10), [2, 3, 1, 0, 9]);
        assert_eq!(one_key_away(0, 3), [0, 1, 2]);
        assert!(one_key_away(0, 0).is_empty());
    }

    #[test]
    fn moving_in_an_empty_list_stays_at_zero() {
        assert_eq!(next_index(0, 0), 0);
        assert_eq!(previous_index(0, 0), 0);
    }
}
