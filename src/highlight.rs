//! Syntax highlighting for the diff, with syntect. Colours whole files, so
//! that each line gets the colours it has in context.

use ratatui::style::Color;
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

/// The colours of VS Code's default dark theme, Dark Modern, as Zed's
/// "VSCode Dark Modern" theme has them, mapped onto TextMate scopes, which
/// syntect's grammars use. Only its text colours are used.
const THEME: &str = include_str!("../assets/vscode-dark-modern.tmTheme");

/// A line of a file as the diff shows it, with tabs expanded, and the colour
/// of each byte range of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ColoredLine {
    pub text: String,
    pub colors: Vec<(Range<usize>, Color)>,
}

// Loaded once, on first use, as loading them takes a moment.
fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn theme() -> &'static Theme {
    static THEME_LOADED: OnceLock<Theme> = OnceLock::new();
    THEME_LOADED.get_or_init(|| {
        let mut reader = std::io::Cursor::new(THEME);
        ThemeSet::load_from_reader(&mut reader).unwrap_or_default()
    })
}

/// The language of a file, from its extension, its whole name (for names like
/// `Makefile`), or its first line (for scripts with a `#!` line).
fn syntax_for(path: &str, text: &str) -> Option<&'static SyntaxReference> {
    let syntaxes = syntaxes();
    let path = Path::new(path);
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let syntax = syntaxes
        .find_syntax_by_extension(extension)
        .or_else(|| syntaxes.find_syntax_by_extension(name))
        .or_else(|| syntaxes.find_syntax_by_first_line(text.lines().next().unwrap_or_default()))?;
    // Plain text would only swap the terminal's text colour for the theme's.
    (syntax.name != "Plain Text").then_some(syntax)
}

/// Colours the first `line_count` lines of a file. Empty if its language
/// isn't known. Highlighting has to start at the top, as a line's colours
/// depend on what came before it, but it stops at the last line needed.
pub fn highlight(path: &str, text: &str, line_count: usize) -> Vec<ColoredLine> {
    let Some(syntax) = syntax_for(path, text) else {
        return Vec::new();
    };
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut lines = Vec::new();
    for line in LinesWithEndings::from(text).take(line_count) {
        let Ok(regions) = highlighter.highlight_line(line, syntaxes()) else {
            break;
        };
        let mut colored = ColoredLine { text: String::new(), colors: Vec::new() };
        for (style, piece) in regions {
            // Expanded like the diff's text, so that the ranges match it.
            let piece = piece.trim_end_matches(['\n', '\r']).replace('\t', "    ");
            let start = colored.text.len();
            colored.text.push_str(&piece);
            let color = style.foreground;
            colored.colors.push((start..colored.text.len(), Color::Rgb(color.r, color.g, color.b)));
        }
        lines.push(colored);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_cover_each_line_of_a_known_language() {
        let lines = highlight("main.rs", "fn main() {\n\tlet x = 1; // one\n}\n", 10);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].text, "    let x = 1; // one");
        let covered: usize = lines[1].colors.iter().map(|(range, _)| range.len()).sum();
        assert_eq!(covered, lines[1].text.len());
        // A keyword and a comment get different colours.
        let color_at = |at: usize| lines[1].colors.iter().find(|(r, _)| r.contains(&at)).unwrap().1;
        assert_ne!(color_at(4), color_at(18));
    }

    /// The colour of the first piece of `line` whose text is `token`.
    fn color_of(line: &ColoredLine, token: &str) -> Color {
        let start = line.text.find(token).unwrap();
        line.colors.iter().find(|(range, _)| range.contains(&start)).unwrap().1
    }

    #[test]
    fn python_gets_vs_code_dark_modern_colours() {
        let code = "import os\n@cache\nclass Point(Base):\n    def area(self, n=2):\n        return len(self.name)  # x\n";
        let lines = highlight("x.py", code, 10);
        let rgb = |hex: u32| Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        assert_eq!(color_of(&lines[0], "import"), rgb(0xC586C0));
        assert_eq!(color_of(&lines[0], "os"), rgb(0x9CDCFE));
        assert_eq!(color_of(&lines[1], "@"), rgb(0xDCDCAA));
        assert_eq!(color_of(&lines[2], "class"), rgb(0x569CD6));
        assert_eq!(color_of(&lines[2], "Point"), rgb(0x4EC9B0));
        assert_eq!(color_of(&lines[3], "area"), rgb(0xDCDCAA));
        assert_eq!(color_of(&lines[3], "n="), rgb(0x9CDCFE));
        assert_eq!(color_of(&lines[3], "2"), rgb(0xB5CEA8));
        assert_eq!(color_of(&lines[4], "return"), rgb(0xC586C0));
        assert_eq!(color_of(&lines[4], "len"), rgb(0xDCDCAA));
        assert_eq!(color_of(&lines[4], "self"), rgb(0x569CD6));
        assert_eq!(color_of(&lines[4], "name"), rgb(0x9CDCFE));
        assert_eq!(color_of(&lines[4], "# x"), rgb(0x6A9955));
    }

    #[test]
    fn stops_at_the_last_line_needed() {
        assert_eq!(highlight("a.rs", "a\nb\nc\nd\n", 2).len(), 2);
    }

    #[test]
    fn unknown_and_plain_text_files_are_not_colored() {
        assert!(highlight("notes.unknownext", "hello", 10).is_empty());
        assert!(highlight("notes.txt", "hello", 10).is_empty());
    }

    #[test]
    fn a_script_is_recognised_by_its_first_line() {
        assert!(!highlight("run", "#!/bin/sh\necho hi\n", 10).is_empty());
    }
}
