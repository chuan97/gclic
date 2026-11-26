//! A command's output as it would show in a terminal: its colour codes as
//! styles, and progress it redrew in place reduced to where it ended.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// A command's output as lines for the screen, still holding its colour
/// codes. Progress that git redraws in place, with carriage returns, keeps
/// only its last state.
pub fn clean_output(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let has_text = |part: &str| !strip_ansi(part).trim().is_empty();
    let lines = text.lines().map(|line| {
        let line = line.trim_end_matches('\r');
        let last = line.rsplit('\r').find(|part| has_text(part)).unwrap_or("");
        last.replace('\t', "    ")
    });
    let mut lines: Vec<String> = lines.collect();
    while lines.last().is_some_and(|line| !has_text(line)) {
        lines.pop();
    }
    lines
}

/// The pieces of a line: text, and the escape codes between it. Codes are
/// `ESC [ parameters final`, and only those ending in `m` set the style.
enum Piece<'a> {
    Text(&'a str),
    Style(&'a str),
    /// Any other code, such as erasing the rest of the line, which a popup has
    /// no use for.
    Other,
}

fn pieces(line: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('\x1b') {
        if start > 0 {
            pieces.push(Piece::Text(&rest[..start]));
        }
        let code = &rest[start + 1..];
        let Some(params) = code.strip_prefix('[') else {
            // An escape that isn't a CSI code: it and its next character go.
            let skip = code.chars().next().map_or(0, char::len_utf8);
            rest = &code[skip..];
            pieces.push(Piece::Other);
            continue;
        };
        // The code ends at its first character from @ to ~.
        match params.find(|ch: char| ('@'..='~').contains(&ch)) {
            Some(end) => {
                let is_style = params[end..].starts_with('m');
                pieces.push(if is_style {
                    Piece::Style(&params[..end])
                } else {
                    Piece::Other
                });
                rest = &params[end + 1..];
            }
            None => {
                rest = "";
                pieces.push(Piece::Other);
            }
        }
    }
    if !rest.is_empty() {
        pieces.push(Piece::Text(rest));
    }
    pieces
}

/// A line's text without its escape codes.
pub fn strip_ansi(line: &str) -> String {
    pieces(line)
        .into_iter()
        .filter_map(|piece| {
            if let Piece::Text(text) = piece {
                Some(text)
            } else {
                None
            }
        })
        .collect()
}

/// A line with its colour codes turned into styles.
pub fn ansi_line(line: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    for piece in pieces(line) {
        match piece {
            Piece::Text(text) => spans.push(Span::styled(text.to_string(), style)),
            Piece::Style(params) => style = apply_sgr(style, params),
            Piece::Other => {}
        }
    }
    Line::from(spans)
}

/// The style after a "select graphic rendition" code's parameters, such as
/// `1;31` for bold red. The common ones: resets, bold, dim, italic,
/// underline, reverse, the 16 colours, and 256 and true colours.
fn apply_sgr(mut style: Style, params: &str) -> Style {
    use ratatui::style::Modifier;
    let numbers: Vec<u16> = params.split(';').map(|n| n.parse().unwrap_or(0)).collect();
    let basic = |n: u16| {
        [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
        ][n as usize]
    };
    let bright = |n: u16| {
        [
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ][n as usize]
    };
    let mut i = 0;
    while i < numbers.len() {
        let n = numbers[i];
        // 38 and 48 take a colour from the parameters after them.
        let mut extended = || match numbers.get(i + 1) {
            Some(5) => {
                let color = numbers.get(i + 2).map(|&c| Color::Indexed(c as u8));
                i += 2;
                color
            }
            Some(2) => {
                let channel = |k: usize| numbers.get(i + k).map(|&c| c as u8);
                let color = match (channel(2), channel(3), channel(4)) {
                    (Some(r), Some(g), Some(b)) => Some(Color::Rgb(r, g, b)),
                    _ => None,
                };
                i += 4;
                color
            }
            _ => None,
        };
        style = match n {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            30..=37 => style.fg(basic(n - 30)),
            38 => extended().map_or(style, |color| style.fg(color)),
            39 => style.fg(Color::Reset),
            40..=47 => style.bg(basic(n - 40)),
            48 => extended().map_or(style, |color| style.bg(color)),
            49 => style.bg(Color::Reset),
            90..=97 => style.fg(bright(n - 90)),
            100..=107 => style.bg(bright(n - 100)),
            _ => style,
        };
        i += 1;
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_codes_become_styles() {
        let line = ansi_line("\x1b[33mabc1234\x1b[m \x1b[1;32mmain\x1b[0m done");
        let pieces: Vec<_> = line
            .spans
            .iter()
            .map(|span| (span.content.as_ref(), span.style))
            .collect();
        assert_eq!(
            pieces,
            [
                ("abc1234", Style::default().fg(Color::Yellow)),
                (" ", Style::default()),
                (
                    "main",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(ratatui::style::Modifier::BOLD)
                ),
                (" done", Style::default()),
            ]
        );
    }

    #[test]
    fn extended_colours_and_other_codes() {
        let line = ansi_line("\x1b[38;5;208mo\x1b[48;2;1;2;3mx\x1b[Kz");
        assert_eq!(line.spans[0].style.fg, Some(Color::Indexed(208)));
        assert_eq!(line.spans[1].style.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(strip_ansi("remote: done\x1b[K"), "remote: done");
        assert_eq!(strip_ansi("\x1b[1mcut\x1b["), "cut");
    }

    #[test]
    fn redrawn_progress_keeps_its_last_state() {
        let output =
            b"Receiving objects:  50% (1/2)\rReceiving objects: 100% (2/2), done.\r\nDone\n\n";
        assert_eq!(
            clean_output(output),
            ["Receiving objects: 100% (2/2), done.", "Done"]
        );
    }
}
