//! The colours of the editor gclic runs inside, for its diffs, so code looks
//! as it does in the editor: its background and text, its diff colours, and
//! its syntax colours. The plugin gives gclic's own interface the colour
//! scheme's too: its palette through Neovim's terminal, the selection's in
//! `$GCLIC_SELECTED_BG`, and its borders', git's and diagnostics' colours
//! in `$GCLIC_UI_COLORS`.
//!
//! Neovim's plugin passes them in `$GCLIC_EDITOR_COLORS`, as `group:part=#rrggbb`
//! pairs separated by `;`, such as `Normal:bg=#1f1f1f;@keyword:fg=#569cd6`.
//! Without it, diffs keep the VS Code Dark Modern colours gclic comes with.

use ratatui::style::Color;
use std::str::FromStr;
use std::sync::OnceLock;
use syntect::highlighting::{
    Color as SyntectColor, ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings,
};

/// The variable the colours come in.
pub const VARIABLE: &str = "GCLIC_EDITOR_COLORS";

/// A colour of the editor's: its group and part, `("@keyword", "fg")`, and
/// the colour.
type Entry = ((String, String), (u8, u8, u8));

/// The editor's colours.
pub struct EditorColors {
    colors: Vec<Entry>,
}

impl EditorColors {
    /// Reads `group:part=#rrggbb;…`, skipping anything it can't read.
    pub fn parse(text: &str) -> EditorColors {
        let colors = text
            .split(';')
            .filter_map(|pair| {
                let (key, value) = pair.split_once('=')?;
                let (group, part) = key.trim().rsplit_once(':')?;
                let rgb = crate::accent::parse_hex(value.trim())?;
                Some(((group.to_string(), part.to_string()), rgb))
            })
            .collect();
        EditorColors { colors }
    }

    fn rgb(&self, group: &str, part: &str) -> Option<(u8, u8, u8)> {
        self.colors
            .iter()
            .find(|((g, p), _)| g == group && p == part)
            .map(|(_, rgb)| *rgb)
    }

    fn color(&self, group: &str, part: &str) -> Option<Color> {
        self.rgb(group, part).map(|(r, g, b)| Color::Rgb(r, g, b))
    }
}

/// The editor's colours, if gclic runs inside one that passed them.
pub fn editor_colors() -> Option<&'static EditorColors> {
    static COLORS: OnceLock<Option<EditorColors>> = OnceLock::new();
    COLORS
        .get_or_init(|| {
            let text = std::env::var(VARIABLE).ok()?;
            let colors = EditorColors::parse(&text);
            (!colors.colors.is_empty()).then_some(colors)
        })
        .as_ref()
}

/// The colours a diff is drawn in, around its syntax colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiffColors {
    /// Behind the code, and the code's own colour where it has no other.
    pub background: Color,
    pub text: Color,
    /// Behind removed and added lines.
    pub removed: Color,
    pub added: Color,
    /// Behind the words that changed, a stronger shade of their line's.
    pub removed_word: Color,
    pub added_word: Color,
}

/// gclic's own: VS Code Dark Modern's.
pub const BUILT_IN: DiffColors = DiffColors {
    background: Color::Rgb(31, 31, 31),
    text: Color::Rgb(204, 204, 204),
    removed: Color::Rgb(58, 30, 34),
    added: Color::Rgb(28, 52, 38),
    removed_word: Color::Rgb(112, 48, 54),
    added_word: Color::Rgb(46, 96, 62),
};

/// A mix of two colours, `amount` of the way from `a` to `b`.
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), amount: f64) -> (u8, u8, u8) {
    let channel = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * amount).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

fn to_color((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

/// How far apart two colours are, as a straight line between them.
fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let d = |x: u8, y: u8| (x as f64 - y as f64).powi(2);
    (d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)).sqrt()
}

fn rgb_of(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    }
}

/// `color`'s hue, `strength` away from `background`, as gclic's own tints
/// are from theirs: never further than `color` itself.
fn tint(background: (u8, u8, u8), color: (u8, u8, u8), strength: f64) -> (u8, u8, u8) {
    let full = distance(background, color);
    if full == 0.0 {
        return color;
    }
    mix(background, color, (strength / full).min(1.0))
}

impl DiffColors {
    /// The editor's, where it has them, and gclic's own otherwise. Colour
    /// schemes' diff colours are opaque, often strong enough to drown the code,
    /// where editors such as VS Code lay theirs over it translucent. So gclic
    /// takes their hue at its own strengths: removed and added lines as far
    /// from the background as its own tints, and changed words as far as its
    /// own words, never further than the colour scheme's own colour.
    pub fn from_editor(editor: &EditorColors) -> DiffColors {
        let background = editor.rgb("Normal", "bg");
        let built_in_bg = rgb_of(BUILT_IN.background);
        let strength = |own: Color| distance(built_in_bg, rgb_of(own));
        let shade = |group: &str, own: Color| {
            let line = editor.rgb(group, "bg")?;
            Some(to_color(tint(
                background.unwrap_or(built_in_bg),
                line,
                strength(own),
            )))
        };
        DiffColors {
            background: editor.color("Normal", "bg").unwrap_or(BUILT_IN.background),
            text: editor.color("Normal", "fg").unwrap_or(BUILT_IN.text),
            removed: shade("DiffDelete", BUILT_IN.removed).unwrap_or(BUILT_IN.removed),
            added: shade("DiffAdd", BUILT_IN.added).unwrap_or(BUILT_IN.added),
            removed_word: shade("DiffDelete", BUILT_IN.removed_word)
                .unwrap_or(BUILT_IN.removed_word),
            added_word: shade("DiffAdd", BUILT_IN.added_word).unwrap_or(BUILT_IN.added_word),
        }
    }
}

/// The variable the colours of gclic's interface come in, inside Neovim, in
/// the same form as the diff's: its window borders, its git colours and its
/// diagnostics'. Without it, gclic keeps its own.
pub const UI_VARIABLE: &str = "GCLIC_UI_COLORS";

/// The colours of gclic's interface, by what they're for, each from the
/// editor's group for the same thing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UiColors {
    /// A pane's border: as Neovim's windows are divided, and, focused, as
    /// its floating windows are outlined, or in its text's colour.
    pub border: Option<Color>,
    pub focused_border: Option<Color>,
    /// Files and lines added, changed and removed, as the sign column marks
    /// them, or as Neovim's own git colours have them.
    pub added: Option<Color>,
    pub changed: Option<Color>,
    pub removed: Option<Color>,
    /// Errors and destructive keys, and warnings.
    pub error: Option<Color>,
    pub warning: Option<Color>,
}

impl UiColors {
    pub fn from_editor(editor: &EditorColors) -> UiColors {
        let first = |groups: &[&str]| groups.iter().find_map(|group| editor.color(group, "fg"));
        UiColors {
            border: first(&["WinSeparator"]),
            focused_border: first(&["FloatBorder", "Normal"]),
            added: first(&["GitSignsAdd", "Added"]),
            changed: first(&["GitSignsChange", "Changed"]),
            removed: first(&["GitSignsDelete", "Removed"]),
            error: first(&["DiagnosticError"]),
            warning: first(&["DiagnosticWarn"]),
        }
    }

    const NONE: UiColors = UiColors {
        border: None,
        focused_border: None,
        added: None,
        changed: None,
        removed: None,
        error: None,
        warning: None,
    };
}

/// The interface's colours: the editor's, inside Neovim, and none, for
/// gclic's own, elsewhere and in tests.
pub fn ui_colors() -> UiColors {
    static COLORS: OnceLock<UiColors> = OnceLock::new();
    *COLORS.get_or_init(|| {
        if cfg!(test) {
            return UiColors::NONE;
        }
        match std::env::var(UI_VARIABLE) {
            Ok(text) => UiColors::from_editor(&EditorColors::parse(&text)),
            Err(_) => UiColors::NONE,
        }
    })
}

/// The diff's colours: the editor's, inside one, or gclic's own.
pub fn diff_colors() -> DiffColors {
    static COLORS: OnceLock<DiffColors> = OnceLock::new();
    *COLORS.get_or_init(|| editor_colors().map_or(BUILT_IN, DiffColors::from_editor))
}

/// The kinds of token syntect's grammars name, as TextMate scope selectors,
/// and the Neovim group that colours each. The plugin, `lua/gclic.lua`,
/// sends these groups, and its list mirrors this one, which a test checks. Where several selectors match a
/// token, the most specific wins, so the broad ones are fallbacks.
pub const SCOPES: &[(&str, &str)] = &[
    ("comment", "@comment"),
    ("string", "@string"),
    ("string.regexp", "@string.regexp"),
    ("constant.character.escape", "@string.escape"),
    ("constant", "@constant"),
    ("constant.numeric", "@number"),
    ("constant.language", "@boolean"),
    ("support.constant", "@constant.builtin"),
    ("keyword", "@keyword"),
    ("storage", "@keyword"),
    (
        "storage.type.function, keyword.declaration.function",
        "@keyword.function",
    ),
    (
        "keyword.control.import, keyword.control.export",
        "@keyword.import",
    ),
    ("keyword.control.return", "@keyword.return"),
    ("keyword.operator", "@operator"),
    (
        "entity.name.function, meta.function-call.generic",
        "@function",
    ),
    ("support.function", "@function.builtin"),
    ("variable.function", "@function.call"),
    (
        "entity.name.function.macro, support.macro",
        "@function.macro",
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.other.inherited-class",
        "@type",
    ),
    ("support.type, support.class", "@type.builtin"),
    ("variable", "@variable"),
    ("variable.parameter", "@variable.parameter"),
    ("variable.language", "@variable.builtin"),
    (
        "variable.other.member, variable.other.property, meta.property-name",
        "@property",
    ),
    ("entity.name.namespace, entity.name.module", "@module"),
    (
        "meta.attribute, meta.annotation, entity.name.function.decorator",
        "@attribute",
    ),
    ("entity.name.tag", "@tag"),
    ("entity.other.attribute-name", "@tag.attribute"),
    ("punctuation", "@punctuation"),
];

/// A syntect theme in the editor's syntax colours, for highlighting. Tokens
/// the editor has no colour for take its text colour.
pub fn syntax_theme(editor: &EditorColors) -> Theme {
    let color = |(r, g, b): (u8, u8, u8)| SyntectColor { r, g, b, a: 255 };
    let scopes = SCOPES
        .iter()
        .filter_map(|(selectors, group)| {
            Some(ThemeItem {
                scope: ScopeSelectors::from_str(selectors).ok()?,
                style: StyleModifier {
                    foreground: Some(color(editor.rgb(group, "fg")?)),
                    background: None,
                    font_style: None,
                },
            })
        })
        .collect();
    Theme {
        settings: ThemeSettings {
            foreground: editor.rgb("Normal", "fg").map(color),
            background: editor.rgb("Normal", "bg").map(color),
            ..ThemeSettings::default()
        },
        scopes,
        ..Theme::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neovim() -> EditorColors {
        EditorColors::parse(
            "Normal:fg=#d4d4d4;Normal:bg=#1e1e1e;DiffAdd:bg=#373d29;DiffDelete:bg=#4b1818;\
             @keyword:fg=#569cd6;@string:fg=#ce9178;@function:fg=#dcdcaa;broken;Nope:fg=red",
        )
    }

    #[test]
    fn reads_group_part_and_colour_and_skips_the_rest() {
        let colors = neovim();
        assert_eq!(colors.rgb("Normal", "bg"), Some((0x1e, 0x1e, 0x1e)));
        assert_eq!(colors.rgb("@keyword", "fg"), Some((0x56, 0x9c, 0xd6)));
        assert_eq!(colors.rgb("Nope", "fg"), None);
        assert_eq!(colors.colors.len(), 7);
    }

    #[test]
    fn a_diff_takes_the_editor_s_colours_and_falls_back_to_gclic_s() {
        let diff = DiffColors::from_editor(&neovim());
        assert_eq!(diff.background, Color::Rgb(0x1e, 0x1e, 0x1e));
        let bare = DiffColors::from_editor(&EditorColors::parse("Normal:bg=#000000"));
        assert_eq!(
            (bare.removed, bare.removed_word, bare.text),
            (BUILT_IN.removed, BUILT_IN.removed_word, BUILT_IN.text)
        );
    }

    #[test]
    fn a_colour_scheme_s_diff_colours_are_taken_at_gclic_s_strengths() {
        // vscode.nvim's, opaque and strong, where VS Code's are translucent.
        let vscode = EditorColors::parse(
            "Normal:bg=#1f1f1f;Normal:fg=#d4d4d4;DiffAdd:bg=#4b5632;DiffDelete:bg=#6f1313",
        );
        let diff = DiffColors::from_editor(&vscode);
        let bg = (0x1f, 0x1f, 0x1f);
        let near = |a: f64, b: f64| (a - b).abs() < 1.5;
        // Lines as faint as gclic's own, in the colour scheme's hue.
        let own = distance(rgb_of(BUILT_IN.background), rgb_of(BUILT_IN.added));
        assert!(
            near(distance(bg, rgb_of(diff.added)), own),
            "{:?}",
            diff.added
        );
        let (r, g, b) = rgb_of(diff.added);
        assert!(g > r && r > b, "still olive: {:?}", diff.added);
        // Words as strong as gclic's, which here is the colour scheme's own.
        assert_eq!(diff.added_word, Color::Rgb(0x4b, 0x56, 0x32));
        assert_eq!(diff.removed_word, Color::Rgb(0x6f, 0x13, 0x13));
        // A faint colour scheme isn't made any stronger.
        let faint = EditorColors::parse("Normal:bg=#1f1f1f;DiffAdd:bg=#222822");
        let diff = DiffColors::from_editor(&faint);
        assert_eq!(
            (diff.added, diff.added_word),
            (Color::Rgb(0x22, 0x28, 0x22), Color::Rgb(0x22, 0x28, 0x22))
        );
    }

    #[test]
    fn the_interface_takes_the_editor_s_colours_for_the_same_things() {
        let editor = EditorColors::parse(
            "WinSeparator:fg=#444444;Normal:fg=#d4d4d4;Added:fg=#b3f6c0;GitSignsAdd:fg=#6a9955;\
             Removed:fg=#ffc0b9;DiagnosticError:fg=#f44747",
        );
        let ui = UiColors::from_editor(&editor);
        assert_eq!(ui.border, Some(Color::Rgb(0x44, 0x44, 0x44)));
        // No FloatBorder: the text's colour.
        assert_eq!(ui.focused_border, Some(Color::Rgb(0xd4, 0xd4, 0xd4)));
        // The sign column's git colours first, Neovim's own after.
        assert_eq!(ui.added, Some(Color::Rgb(0x6a, 0x99, 0x55)));
        assert_eq!(ui.removed, Some(Color::Rgb(0xff, 0xc0, 0xb9)));
        assert_eq!(ui.error, Some(Color::Rgb(0xf4, 0x47, 0x47)));
        // What the editor doesn't say, gclic keeps its own for.
        assert_eq!((ui.changed, ui.warning), (None, None));
        assert_eq!(ui_colors(), UiColors::NONE, "never in tests");
    }

    #[test]
    fn code_is_highlighted_in_the_editor_s_syntax_colours() {
        let theme = syntax_theme(&neovim());
        let syntaxes = syntect::parsing::SyntaxSet::load_defaults_newlines();
        let rust = syntaxes.find_syntax_by_extension("rs").unwrap();
        let mut highlighter = syntect::easy::HighlightLines::new(rust, &theme);
        let line = "fn main() { let s = \"hi\"; }\n";
        let regions = highlighter.highlight_line(line, &syntaxes).unwrap();
        let color_of = |word: &str| {
            let style = regions
                .iter()
                .find(|(_, piece)| piece.contains(word))
                .unwrap()
                .0;
            (style.foreground.r, style.foreground.g, style.foreground.b)
        };
        assert_eq!(color_of("fn"), (0x56, 0x9c, 0xd6)); // @keyword, for want of @keyword.function
        assert_eq!(color_of("main"), (0xdc, 0xdc, 0xaa)); // @function
        assert_eq!(color_of("hi"), (0xce, 0x91, 0x78)); // @string
    }

    /// The plugin's lists of groups to send, `DIFF_GROUPS` and `SYNTAX_GROUPS`
    /// in `lua/gclic.lua`, are the groups gclic reads: a group missing from
    /// them would keep the text colour without anything failing.
    #[test]
    fn the_plugin_sends_the_groups_gclic_reads() {
        let lua = include_str!("../lua/gclic.lua");
        let list = |name: &str| -> Vec<&str> {
            let start = lua
                .find(&format!("local {name} = {{"))
                .unwrap_or_else(|| panic!("{name} not in lua/gclic.lua"));
            let rest = &lua[start..];
            let body = &rest[rest.find('{').unwrap() + 1..];
            // The list ends at the first `}` after the last `{`, so nested pairs count.
            let mut depth = 1;
            let end = body
                .char_indices()
                .find(|&(_, c)| {
                    depth += match c {
                        '{' => 1,
                        '}' => -1,
                        _ => 0,
                    };
                    depth == 0
                })
                .unwrap()
                .0;
            body[..end].split('"').skip(1).step_by(2).collect()
        };
        let diff: Vec<_> = list("DIFF_GROUPS")
            .chunks(2)
            .map(|pair| (pair[0], pair[1]))
            .collect();
        assert_eq!(
            diff,
            [
                ("Normal", "fg"),
                ("Normal", "bg"),
                ("DiffAdd", "bg"),
                ("DiffDelete", "bg")
            ]
        );
        let mut syntax: Vec<&str> = Vec::new();
        for (_, group) in SCOPES {
            if !syntax.contains(group) {
                syntax.push(group);
            }
        }
        assert_eq!(list("SYNTAX_GROUPS"), syntax);
    }
}
