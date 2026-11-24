//! Small drawing helpers shared by the panels.

use crate::git::{DiffSource, FileEntry, Lines};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear};

pub const SELECTED_BG: Color = Color::Indexed(237);
pub const DIM: Color = Color::DarkGray;

/// A list's row, highlighted if it's the selected one: on the selection's
/// background, so the row stays marked when the list isn't focused, and bold
/// while it is.
pub fn selected(line: Line<'_>, selected: bool, focused: bool) -> Line<'_> {
    match (selected, focused) {
        (false, _) => line,
        (true, false) => line.style(Style::default().bg(SELECTED_BG)),
        (true, true) => line.style(Style::default().bg(SELECTED_BG).add_modifier(Modifier::BOLD)),
    }
}
/// What a set of changes adds and removes together: the files' counts summed,
/// binary only if they all are, and unknown if none is known.
pub fn total_lines(lines: impl IntoIterator<Item = Lines>) -> Lines {
    let mut total = Lines::Unknown;
    for lines in lines {
        total = match (total, lines) {
            (Lines::Counted { added, removed }, Lines::Counted { added: a, removed: r }) => {
                Lines::Counted { added: added + a, removed: removed + r }
            }
            (Lines::Counted { .. }, _) => total,
            (_, Lines::Counted { .. }) => lines,
            (Lines::Binary, _) | (_, Lines::Binary) => Lines::Binary,
            _ => Lines::Unknown,
        };
    }
    total
}

/// `+12 −3`, in green and red. Nothing for lines not known, and "binary"
/// for a binary file.
pub fn line_count_spans(lines: Lines) -> Vec<Span<'static>> {
    match lines {
        Lines::Unknown => Vec::new(),
        Lines::Binary => vec![Span::styled("binary", Style::default().fg(DIM))],
        Lines::Counted { added, removed } => vec![
            Span::styled(format!("+{added}"), Style::default().fg(Color::Green)),
            Span::raw(" "),
            Span::styled(format!("−{removed}"), Style::default().fg(Color::Red)),
        ],
    }
}

/// `line`, then the lines its change adds and removes, two spaces after.
pub fn with_line_counts(mut line: Line<'_>, lines: Lines) -> Line<'_> {
    let counts = line_count_spans(lines);
    if !counts.is_empty() {
        line.spans.push(Span::raw("  "));
        line.spans.extend(counts);
    }
    line
}

/// A list of files' changes in all: `+34 −41 · 2 files`.
pub fn files_summary(files: &[FileEntry]) -> Vec<Span<'static>> {
    let mut spans = line_count_spans(total_lines(files.iter().map(|file| file.lines)));
    if !spans.is_empty() {
        spans.push(Span::styled(" · ", Style::default().fg(DIM)));
    }
    let noun = if files.len() == 1 { "file" } else { "files" };
    spans.push(Span::styled(format!("{} {noun}", files.len()), Style::default().fg(DIM)));
    spans
}

/// `gg` to go to the top, the same everywhere: the first g waits, kept in
/// `pending`, and the second comes back as the one g to handle. Any other key
/// after a g is dropped. None while there's no key to handle yet. The panes
/// wait for theirs in the app, as their g also starts gc and gC.
pub fn gg(pending: &mut bool, key: KeyEvent) -> Option<KeyEvent> {
    let is_g = key.code == KeyCode::Char('g') && key.modifiers.is_empty();
    if std::mem::take(pending) {
        return is_g.then_some(key);
    }
    if is_g {
        *pending = true;
        return None;
    }
    Some(key)
}

/// A cursor in a list, and the scroll that keeps it on screen. j and k wrap
/// around the ends, and g and G go to them, the same in every list.
#[derive(Default)]
pub struct Cursor {
    pub index: usize,
    /// The first row drawn.
    scroll: usize,
}

impl Cursor {
    pub fn down(&mut self, len: usize) {
        self.index = next_index(self.index, len);
    }

    pub fn up(&mut self, len: usize) {
        self.index = previous_index(self.index, len);
    }

    /// Moves with j/k and the arrows, wrapping, and g/G, in a list of `len`.
    /// Whether the key was one of them.
    pub fn handle_key(&mut self, key: KeyEvent, len: usize) -> bool {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.down(len),
            KeyCode::Char('k') | KeyCode::Up => self.up(len),
            KeyCode::Char('g') => self.index = 0,
            KeyCode::Char('G') => self.index = len.saturating_sub(1),
            _ => return false,
        }
        true
    }

    /// Keeps the cursor in a list of `len`, as after it got shorter.
    pub fn clamp(&mut self, len: usize) {
        self.index = self.index.min(len.saturating_sub(1));
    }

    /// The first row to draw, `height` rows high, for the cursor's `row` to
    /// show, scrolling as little as possible. The row differs from the index
    /// where headings go between the items.
    pub fn scroll_to(&mut self, row: usize, height: usize) -> usize {
        self.scroll = keep_visible(self.scroll, row, height);
        self.scroll
    }

    /// The positions one key away in a list of `len`, most likely first: the
    /// cursor's, then where j, k, g and G lead. Used to prefetch diffs.
    pub fn one_key_away(&self, len: usize) -> Vec<usize> {
        one_key_away(self.index, len)
    }
}

/// Scrolling through text taller than its area, as the help, a `:git`
/// command's output and a commit's details do. How far it can go is known
/// once drawn, from `fit`.
#[derive(Default)]
pub struct Scroll {
    /// The first line shown.
    offset: usize,
    /// Rows of text on screen.
    page: usize,
    /// The furthest it can scroll.
    max: usize,
}

impl Scroll {
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Scrolls with j/k and the arrows, ctrl-d/u by half a page, and g/G to
    /// the ends. False for any other key.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let half_page = (self.page / 2).max(1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.offset = (self.offset + 1).min(self.max),
            KeyCode::Char('k') | KeyCode::Up => self.offset = self.offset.saturating_sub(1),
            KeyCode::Char('d') if ctrl => self.offset = (self.offset + half_page).min(self.max),
            KeyCode::Char('u') if ctrl => self.offset = self.offset.saturating_sub(half_page),
            KeyCode::Char('g') => self.offset = 0,
            KeyCode::Char('G') => self.offset = self.max,
            _ => return false,
        }
        true
    }

    /// Takes the rows on screen, `page`, and the text's, `total`, as drawn,
    /// keeping the offset within them.
    pub fn fit(&mut self, page: usize, total: usize) {
        self.page = page;
        self.max = total.saturating_sub(page);
        self.offset = self.offset.min(self.max);
    }
}

/// A popup of text that scrolls when it doesn't fit, such as the help: drawn
/// over what's at `popup`, in `block`, whose bottom border says how to
/// scroll it and that any other key closes it.
pub fn text_popup(frame: &mut Frame, popup: Rect, block: Block, lines: Vec<Line>, scroll: &mut Scroll) {
    scroll.fit(popup.height.saturating_sub(2) as usize, lines.len());
    let closing = if scroll.max > 0 {
        key_hints(&[("j/k", "scroll"), ("any other key", "closes")], None)
    } else {
        key_hints(&[("any key", "closes")], None)
    };
    frame.render_widget(Clear, popup);
    let block = block.title_bottom(closing);
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    render_lines(frame, inner, lines.into_iter().skip(scroll.offset).collect());
}

/// The colour of keys wherever they're listed: popups and the help.
pub const KEY: Color = Color::Yellow;

/// A key and what it does, as the status line and the popups list them. An
/// empty key is for words with no key, as in "type to filter".
pub type Hint<'a> = (&'a str, &'a str);

/// Enter on a file shows its diff, or moves into the diff if it's the one
/// showing, `shown`.
pub fn enter_diff_hint(selected: Option<DiffSource>, shown: Option<&DiffSource>) -> Option<Hint<'static>> {
    let selected = selected?;
    Some(if shown == Some(&selected) { ("enter", "focus diff") } else { ("enter", "diff") })
}

/// Keys and what they do, for a popup's bottom border: keys in `KEY` and
/// actions in the normal colour. The key for the action named `danger`, if
/// any, is bold red instead, as it's the key that's about to be pressed.
pub fn key_hints(hints: &[(&str, &str)], danger: Option<&str>) -> Line<'static> {
    // Nothing at all, as even padding would cut into the border.
    if hints.is_empty() {
        return Line::default();
    }
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
fn one_key_away(selected: usize, len: usize) -> Vec<usize> {
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

/// A line of text broken into lines of at most `width` characters, at
/// spaces where it can be.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        let mut word = word.to_string();
        // A word too long for a line of its own is cut.
        while word.chars().count() > width {
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            let cut = word.char_indices().nth(width).map_or(word.len(), |(i, _)| i);
            lines.push(word[..cut].to_string());
            word = word[cut..].to_string();
        }
        let needed = if line.is_empty() { word.chars().count() } else { line.chars().count() + 1 + word.chars().count() };
        if needed > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    lines.push(line);
    lines
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

/// The colour of a file's status letter, as VS Code's Dark Modern theme
/// colours files in its git panel. The letters stay git's own.
pub fn status_color(status: char) -> Color {
    match status {
        'A' => Color::Rgb(0x81, 0xB8, 0x8B), // added
        '?' => Color::Rgb(0x73, 0xC9, 0x91), // untracked
        'D' => Color::Rgb(0xC7, 0x4E, 0x39), // deleted
        'U' => Color::Rgb(0xE4, 0x67, 0x6B), // conflicted
        _ => Color::Rgb(0xE2, 0xC0, 0x8D),   // modified, and anything else
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_wraps_at_spaces_and_cuts_words_too_long_for_a_line() {
        assert_eq!(wrap("Is thirty seconds enough", 12), ["Is thirty", "seconds", "enough"]);
        assert_eq!(wrap("", 12), [""]);
        assert_eq!(wrap("a abcdefghijklmnop b", 10), ["a", "abcdefghij", "klmnop b"]);
    }


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
    fn no_hints_leave_the_border_whole() {
        assert_eq!(key_hints(&[], None).width(), 0);
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
        let mut cursor = Cursor::default();
        for key in ['j', 'k', 'g', 'G'] {
            assert!(cursor.handle_key(KeyEvent::from(KeyCode::Char(key)), 0));
            assert_eq!(cursor.index, 0);
        }
    }

    #[test]
    fn totals_add_the_known_counts_and_ignore_the_rest() {
        let counted = |added, removed| Lines::Counted { added, removed };
        assert_eq!(total_lines([counted(3, 1), Lines::Binary, counted(2, 0)]), counted(5, 1));
        assert_eq!(total_lines([Lines::Binary, Lines::Unknown]), Lines::Binary);
        assert_eq!(total_lines([]), Lines::Unknown);
    }

    #[test]
    fn counts_follow_the_name() {
        let text = |line: Line| line.spans.iter().map(|span| span.content.to_string()).collect::<String>();
        let counted = Lines::Counted { added: 4, removed: 3 };
        assert_eq!(text(with_line_counts(Line::raw(" M a.rs"), counted)), " M a.rs  +4 −3");
        assert_eq!(text(with_line_counts(Line::raw(" M a.png"), Lines::Binary)), " M a.png  binary");
        assert_eq!(text(with_line_counts(Line::raw(" M a.rs"), Lines::Unknown)), " M a.rs");
    }

    #[test]
    fn gg_waits_for_the_second_g_and_drops_anything_else() {
        let mut pending = false;
        let g = KeyEvent::from(KeyCode::Char('g'));
        let j = KeyEvent::from(KeyCode::Char('j'));
        assert_eq!(gg(&mut pending, j), Some(j));
        assert_eq!(gg(&mut pending, g), None);
        assert_eq!(gg(&mut pending, g), Some(g));
        assert_eq!(gg(&mut pending, g), None);
        assert_eq!(gg(&mut pending, j), None);
        assert!(!pending);
    }

    #[test]
    fn a_cursor_wraps_with_j_and_k_and_takes_only_its_keys() {
        let mut cursor = Cursor::default();
        let mut press = |key| cursor.handle_key(KeyEvent::from(key), 3);
        assert!(press(KeyCode::Char('k')));
        assert!(press(KeyCode::Down));
        assert!(!press(KeyCode::Enter));
        assert_eq!(cursor.index, 0);
        cursor.handle_key(KeyEvent::from(KeyCode::Char('G')), 3);
        assert_eq!(cursor.index, 2);
        cursor.clamp(1);
        assert_eq!(cursor.index, 0);
    }
}
