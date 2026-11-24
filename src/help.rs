//! The help popup: every key, grouped by where it works, with the section for
//! where the user is marked. Two columns when the screen is wide enough, and
//! scrollable when it isn't tall enough.

use crate::ui;
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// A group of keys and what each does, under a heading.
struct Section {
    title: &'static str,
    keys: &'static [(&'static str, &'static str)],
}

/// The left column: the panes on the left of the screen, and the stashes and
/// commands started from them.
const LEFT: &[Section] = &[
    Section {
        title: "Everywhere",
        keys: &[
            ("h l", "previous/next pane"),
            ("j k gg G", "move, stopping at the ends"),
            ("enter", "open; on a file, again to move into it"),
            ("esc", "go back, then close the diff or details"),
            ("o", "open the file in your editor, at the line"),
            ("z", "zoom the diff or details"),
            ("tab", "next tab: Repo, Compare, Review"),
            (":", "a command; tab completes its name"),
            ("?", "this help"),
            ("q ctrl-c", "quit, after asking; ctrl-c stops a :git first"),
        ],
    },
    Section {
        title: "Branches",
        keys: &[
            ("enter", "check out"),
            ("c", "compare with the checked-out branch"),
            ("space", "show its remotes"),
            ("i", "search, or type a sha for the branches with it"),
            ("esc", "clear the search"),
        ],
    },
    Section {
        title: "Changes",
        keys: &[
            ("space", "stage or unstage"),
            ("a A", "stage all, unstage all"),
            ("d", "restore the file, or delete it if new"),
            ("D", "do that to every change not staged"),
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
            ("a p d", "on a stash: apply, pop, drop"),
            ("esc", "collapse"),
        ],
    },
    Section {
        title: "Stashes",
        keys: &[
            (":stash", "list them; :git stash makes one"),
            ("enter", "a stash's files and their diffs"),
            ("a p", "apply it, or apply and drop it"),
            ("d", "drop it, after asking"),
            ("esc", "from its files, back to the list"),
        ],
    },
    Section {
        title: "Commands",
        keys: &crate::app::COMMAND_HELP,
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
        title: "Review",
        keys: &[
            ("gc", "comment on the diff's active line"),
            ("gC", "comment on the diff's whole file"),
            ("tab", "the Review tab, once there are comments"),
            ("enter", "move into the comment's diff"),
            ("c", "edit the comment"),
            ("d", "delete it, after asking"),
            ("y", "copy them all, ready to paste"),
            ("D", "clear the review, after asking"),
        ],
    },
    Section {
        title: "Diff",
        keys: &[
            ("j k", "move the active line"),
            ("gg G", "first/last line"),
            ("ctrl-d u", "half a page down/up"),
            ("n N", "next/previous change, from any pane"),
            ("h l", "scroll sideways"),
            ("e", "whole file, or just the changes, from any pane"),
            ("s", "side by side, or unified, from any pane"),
            ("gc gC", "comment on the line, on the file"),
        ],
    },
    Section {
        title: "Commit details",
        keys: &[("j k gg G", "scroll"), ("ctrl-d u", "half a page down/up"), ("y", "copy the sha")],
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
        title: "Comment",
        keys: &[("ctrl-enter", "save; saved empty, it's removed"), ("esc", "cancel")],
    },
    Section {
        title: "Commit message",
        keys: &[
            ("ctrl-enter", "commit (ctrl-s in some terminals)"),
            ("esc", "close, keeping the draft"),
        ],
    },
];

/// Whether the help lists `key` in one of the sections titled `titles`, alone
/// or in a group such as "a A". A hint's keys joined with /, as in "a/A",
/// must each be listed.
#[cfg(test)]
pub fn lists(titles: &[&str], key: &str) -> bool {
    let listed: Vec<&str> = LEFT
        .iter()
        .chain(RIGHT)
        .filter(|section| titles.contains(&section.title))
        .flat_map(|section| section.keys.iter().flat_map(|(keys, _)| keys.split(' ')))
        .collect();
    key.split('/').all(|key| listed.contains(&key))
}

/// Room for the longest key, so the descriptions line up.
const KEY_WIDTH: usize = 12;
const GAP: usize = 4;

pub struct HelpPopup {
    /// The section for where help was opened from, which gets a marker.
    here: &'static str,
    scroll: ui::Scroll,
}

impl HelpPopup {
    pub fn new(here: &'static str) -> HelpPopup {
        HelpPopup { here, scroll: ui::Scroll::default() }
    }

    /// Scrolls with the usual keys. Any other key closes the popup, which
    /// this returns false for.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        self.scroll.handle_key(key)
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
        let popup = ui::centered(area, width, height);
        ui::text_popup(frame, popup, ui::panel(" Help ".to_string(), true), lines, &mut self.scroll);
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
    use ratatui::crossterm::event::KeyCode;

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
        help.scroll.fit(10, 13);
        assert!(help.handle_key(KeyEvent::from(KeyCode::Char('k'))));
        assert_eq!(help.scroll.offset(), 0);
        help.handle_key(KeyEvent::from(KeyCode::Char('G')));
        help.handle_key(KeyEvent::from(KeyCode::Char('j')));
        assert_eq!(help.scroll.offset(), 3);
        assert!(!help.handle_key(KeyEvent::from(KeyCode::Esc)));
    }
}
