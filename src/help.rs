//! The help popup: every key, grouped by where it works, with the section for
//! where the user is marked. Two columns when the screen is wide enough, and
//! scrollable when it isn't tall enough.

use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;

/// A group of keys and what each does, under a heading.
struct Section {
    title: &'static str,
    keys: &'static [(&'static str, &'static str)],
}

/// The left column: the panes on the left of the screen.
const LEFT: &[Section] = &[
    Section {
        title: "Everywhere",
        keys: &[
            ("h l", "previous/next pane"),
            ("j k g G", "move, wrapping around in lists"),
            ("enter", "open; on a file, again to move into it"),
            ("esc", "go back, then close the diff or details"),
            ("o", "open the file in Zed"),
            ("z", "zoom the diff or details"),
            ("tab", "switch between Repo and Compare"),
            (":", "command line; :cd [name | ..] switches repo"),
            ("?", "this help"),
            ("q", "quit, or ctrl-c at once"),
        ],
    },
    Section {
        title: "Branches",
        keys: &[
            ("enter", "check out"),
            ("c", "compare with the checked-out branch"),
            ("space", "show its remotes"),
            ("i", "search; esc stops typing"),
            ("esc", "clear the search"),
        ],
    },
    Section {
        title: "Changes",
        keys: &[
            ("space", "stage or unstage"),
            ("a A", "stage all, unstage all"),
            ("d", "restore the file, or delete it if new"),
            ("c", "commit what's staged"),
            ("enter", "show the diff"),
        ],
    },
    Section {
        title: "Graph",
        keys: &[
            ("enter", "expand a commit, show a file's diff"),
            ("n N", "next/previous commit, if no diff is open"),
            ("i", "commit details, again to move in"),
            ("c", "compare the commit with a ref"),
            ("y", "copy the commit's sha"),
            ("b", "choose which branches to show"),
            ("esc", "collapse"),
        ],
    },
];

/// The right column: the right side of the screen, and the popups.
const RIGHT: &[Section] = &[
    Section {
        title: "Compare",
        keys: &[
            ("enter", "show the diff, or fold a folder"),
            ("h l", "fold a folder or go up, unfold or go in"),
            ("c", "new comparison: choose two refs"),
            ("r", "swap from and to"),
            ("esc", "close it after confirming, back to Repo"),
        ],
    },
    Section {
        title: "Diff",
        keys: &[
            ("j k", "move the active line"),
            ("g G", "first/last line"),
            ("ctrl-d u", "half a page down/up"),
            ("n N", "next/previous change, from any pane"),
            ("h l", "scroll sideways"),
            ("e", "whole file, or just the changes, from any pane"),
            ("s", "side by side, or unified, from any pane"),
        ],
    },
    Section {
        title: "Commit details",
        keys: &[("j k g G", "scroll"), ("ctrl-d u", "half a page down/up"), ("y", "copy the sha")],
    },
    Section {
        title: "Pickers",
        keys: &[
            ("i", "type to filter; ctrl-j k move"),
            ("space", "tick a branch"),
            ("a A", "tick all shown, untick all"),
            ("p", "paste a ref, when comparing"),
            ("enter", "confirm"),
            ("esc", "stop typing, then close"),
        ],
    },
    Section {
        title: "Commit message",
        keys: &[
            ("ctrl-enter", "commit (ctrl-s in some terminals)"),
            ("esc", "close, keeping the draft"),
        ],
    },
];

/// Room for the longest key, so the descriptions line up.
const KEY_WIDTH: usize = 12;
const GAP: usize = 4;

pub struct HelpPopup {
    /// The section for where help was opened from, which gets a marker.
    here: &'static str,
    scroll: usize,
    /// Rows of text on screen and the furthest scroll, known once drawn.
    page: usize,
    max_scroll: usize,
}

impl HelpPopup {
    pub fn new(here: &'static str) -> HelpPopup {
        HelpPopup { here, scroll: 0, page: 0, max_scroll: 0 }
    }

    /// Scrolls with the usual keys. Any other key closes the popup, which
    /// this returns false for.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let half_page = (self.page / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll = (self.scroll + 1).min(self.max_scroll),
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.scroll = (self.scroll + half_page).min(self.max_scroll),
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(half_page),
            KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('G') => self.scroll = self.max_scroll,
            _ => return false,
        }
        true
    }

    /// A column of sections as lines, each as wide as `width`.
    fn column(&self, sections: &[Section], width: usize) -> Vec<Line<'static>> {
        let heading = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
        let key = Style::default().fg(ui::KEY);
        let mut lines = Vec::new();
        for section in sections {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            let mut title = vec![Span::styled(format!("  {}", section.title), heading)];
            if section.title == self.here {
                title[0] = Span::styled(format!("▸ {}", section.title), heading);
                title.push(Span::styled("  you are here", Style::default().fg(ui::DIM)));
            }
            lines.push(Line::from(title));
            for (keys, description) in section.keys {
                lines.push(Line::from(vec![
                    Span::styled(format!("    {keys:KEY_WIDTH$}"), key),
                    Span::raw(*description),
                ]));
            }
        }
        // Padded, so a second column starts at the same place on every row.
        for line in &mut lines {
            let padding = width.saturating_sub(line.width());
            line.spans.push(Span::raw(" ".repeat(padding)));
        }
        lines
    }

    /// Every line of the help, in two columns if `width` allows.
    fn lines(&self, width: usize) -> Vec<Line<'static>> {
        let column_width = widest(LEFT).max(widest(RIGHT));
        if width < 2 * column_width + GAP {
            let mut lines = self.column(LEFT, column_width);
            lines.push(Line::default());
            lines.extend(self.column(RIGHT, column_width));
            return lines;
        }
        let left = self.column(LEFT, column_width);
        let right = self.column(RIGHT, column_width);
        (0..left.len().max(right.len()))
            .map(|row| {
                let mut spans = left.get(row).cloned().unwrap_or_default().spans;
                if spans.is_empty() {
                    spans.push(Span::raw(" ".repeat(column_width)));
                }
                spans.push(Span::raw(" ".repeat(GAP)));
                spans.extend(right.get(row).cloned().unwrap_or_default().spans);
                Line::from(spans)
            })
            .collect()
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Two columns and the borders, or as much of it as fits.
        let column_width = widest(LEFT).max(widest(RIGHT));
        let width = ((2 * column_width + GAP + 2) as u16).min(area.width.saturating_sub(2));
        let lines = self.lines(width.saturating_sub(2) as usize);
        let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));

        self.page = height.saturating_sub(2) as usize;
        self.max_scroll = lines.len().saturating_sub(self.page);
        self.scroll = self.scroll.min(self.max_scroll);
        let closing = if self.max_scroll > 0 {
            ui::key_hints(&[("j/k", "scroll"), ("any other key", "closes")], None)
        } else {
            ui::key_hints(&[("any key", "closes")], None)
        };

        let popup = ui::centered(area, width, height);
        frame.render_widget(Clear, popup);
        let block = ui::panel(" Help ".to_string(), true).title_bottom(closing);
        let inner = block.inner(popup);
        frame.render_widget(block, popup);
        ui::render_lines(frame, inner, lines.into_iter().skip(self.scroll).collect());
    }
}

/// The width of a column of these sections, at its widest line.
fn widest(sections: &[Section]) -> usize {
    // The marker's "you are here" makes headings longer than they look.
    let heading = |section: &Section| section.title.len() + "▸   you are here".chars().count();
    let entry = |description: &str| 4 + KEY_WIDTH + description.chars().count();
    sections
        .iter()
        .flat_map(|section| {
            let entries = section.keys.iter().map(|(_, description)| entry(description));
            entries.chain([heading(section)])
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn keys_fit_their_column() {
        for section in LEFT.iter().chain(RIGHT) {
            for (keys, _) in section.keys {
                assert!(keys.len() < KEY_WIDTH, "{keys} is too long for the key column");
            }
        }
    }

    #[test]
    fn a_wide_screen_gets_two_columns_and_a_narrow_one_gets_one() {
        let help = HelpPopup::new("Graph");
        let wide = help.lines(200);
        let narrow = help.lines(40);
        assert!(text(&wide[0]).contains("Everywhere") && text(&wide[0]).contains("Compare"));
        assert!(narrow.len() > wide.len());
        assert!(narrow.iter().any(|line| text(line).contains("Commit message")));
    }

    #[test]
    fn the_section_for_where_help_was_opened_is_marked() {
        let lines = HelpPopup::new("Diff").lines(200);
        let marked: Vec<String> = lines.iter().map(text).filter(|line| line.contains('▸')).collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].contains("▸ Diff  you are here"));
    }

    #[test]
    fn the_second_column_starts_at_the_same_place_on_every_row() {
        let lines: Vec<String> = HelpPopup::new("Graph").lines(200).iter().map(text).collect();
        let column = |title: &str| -> usize {
            let line = lines.iter().find(|line| line.contains(title)).unwrap();
            line.chars().count() - line[line.find(title).unwrap()..].chars().count()
        };
        // The headings are indented by two, past the left column and the gap.
        let start = widest(LEFT).max(widest(RIGHT)) + GAP + 2;
        for title in ["Compare", "Diff", "Commit details", "Pickers", "Commit message"] {
            assert_eq!(column(title), start, "{title}");
        }
    }

    #[test]
    fn scrolling_stops_at_the_ends_and_other_keys_close() {
        let mut help = HelpPopup::new("Graph");
        help.max_scroll = 3;
        assert!(help.handle_key(KeyEvent::from(KeyCode::Char('k'))));
        assert_eq!(help.scroll, 0);
        help.handle_key(KeyEvent::from(KeyCode::Char('G')));
        help.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(help.scroll, 3);
        assert!(!help.handle_key(KeyEvent::from(KeyCode::Esc)));
    }
}
