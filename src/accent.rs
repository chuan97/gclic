//! Each repo's colour, for the bar at the top, so repos open side by side
//! are told apart at a glance. By default it comes from the repo's name, so
//! everyone with a clone sees the same one. `:color` sets another, kept in
//! the repo's local git config, which isn't committed, and `:color off`
//! hides the bar in every repo.

use crate::git;
use ratatui::style::Color;

/// Where a custom colour is kept, in `.git/config`.
pub const CONFIG_KEY: &str = "gclic.color";
/// Whether the bar shows, set to false by `:color off` in the user's git
/// config, as someone who'd rather not have it wouldn't want it anywhere.
pub const BAR_KEY: &str = "gclic.colorBar";

/// Whether to show the bar: unless it's turned off.
pub fn bar_shown() -> bool {
    git::config_bool(BAR_KEY) != Some(false)
}

/// Lightness and chroma in OKLCH, the same for every repo, so each hue looks
/// as strong as the others: light enough for black text, and not garish.
const LIGHTNESS: f64 = 0.72;
const CHROMA: f64 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub rgb: (u8, u8, u8),
    /// Set with `:color`, rather than coming from the name.
    pub custom: bool,
}

impl Accent {
    /// The current repo's colour: its custom one, or else its name's.
    pub fn current() -> Accent {
        let custom = git::local_config(CONFIG_KEY).and_then(|value| parse_hex(&value));
        match custom {
            Some(rgb) => Accent { rgb, custom: true },
            None => Accent { rgb: from_name(&repo_name()), custom: false },
        }
    }

    pub fn color(&self) -> Color {
        let (r, g, b) = self.rgb;
        Color::Rgb(r, g, b)
    }

    /// Black or white, whichever contrasts more with the colour. They tie at
    /// a luminance of about 0.18.
    pub fn text(&self) -> Color {
        if luminance(self.rgb) > 0.179 { Color::Black } else { Color::White }
    }

    pub fn hex(&self) -> String {
        let (r, g, b) = self.rgb;
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// The repo's name as everyone with a clone would know it: the last part of
/// origin's URL, or failing that the folder's name. A worktree, or a clone
/// in a folder named otherwise, still gets the same colour.
fn repo_name() -> String {
    git::local_config("remote.origin.url")
        .and_then(|url| name_from_url(&url))
        .or_else(|| {
            let root = std::env::current_dir().ok()?;
            Some(root.file_name()?.to_string_lossy().into_owned())
        })
        .unwrap_or_default()
}

/// `git@github.com:me/gclic.git`, `https://github.com/me/gclic/` and
/// `/srv/git/gclic.git` all give `gclic`.
fn name_from_url(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let last = url.rsplit(['/', ':', '\\']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then(|| name.to_string())
}

/// A colour from a name: its hash picks the hue.
pub fn from_name(name: &str) -> (u8, u8, u8) {
    // FNV-1a, which is stable across versions and platforms, unlike std's hasher.
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in name.to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // FNV's low bits barely change between short names, so mix them all in
    // first (splitmix64's finish), or similar names would get similar hues.
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d049bb133111eb);
    hash ^= hash >> 31;
    let hue = (hash % 360) as f64;
    oklch_to_rgb(LIGHTNESS, CHROMA, hue)
}

/// `#rrggbb` or `rrggbb`, in either case.
pub fn parse_hex(text: &str) -> Option<(u8, u8, u8)> {
    let hex = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if hex.len() != 6 || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

/// OKLCH to sRGB. A colour outside what the screen can show loses chroma
/// until it fits, keeping its hue and lightness.
fn oklch_to_rgb(lightness: f64, chroma: f64, hue: f64) -> (u8, u8, u8) {
    let mut chroma = chroma;
    loop {
        let (a, b) = (chroma * hue.to_radians().cos(), chroma * hue.to_radians().sin());
        let linear = oklab_to_linear(lightness, a, b);
        if linear.iter().all(|c| (0.0..=1.0).contains(c)) || chroma <= 0.0 {
            let [r, g, b] = linear.map(|c| (gamma(c.clamp(0.0, 1.0)) * 255.0).round() as u8);
            return (r, g, b);
        }
        chroma -= 0.005;
    }
}

fn oklab_to_linear(l: f64, a: f64, b: f64) -> [f64; 3] {
    let l_ = (l + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m_ = (l - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s_ = (l - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    [
        4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
        -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
        -0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_,
    ]
}

/// Linear light to sRGB's encoding.
fn gamma(c: f64) -> f64 {
    if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// Relative luminance, as WCAG defines it.
fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let linear = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_comes_from_the_remote_url_in_any_form() {
        for url in ["git@github.com:me/gclic.git", "https://github.com/me/gclic/", "/srv/git/gclic.git", "gclic"] {
            assert_eq!(name_from_url(url).as_deref(), Some("gclic"), "{url}");
        }
        assert_eq!(name_from_url(""), None);
    }

    #[test]
    fn a_name_always_gets_the_same_colour_and_names_differ() {
        assert_eq!(from_name("gclic"), from_name("gclic"));
        assert_eq!(from_name("gclic"), from_name("GCLIC"));
        assert_ne!(from_name("gclic"), from_name("dotfiles"));
    }

    #[test]
    fn colours_from_names_all_take_black_text() {
        for hue in (0..360).step_by(5) {
            let rgb = oklch_to_rgb(LIGHTNESS, CHROMA, f64::from(hue));
            let accent = Accent { rgb, custom: false };
            assert_eq!(accent.text(), Color::Black, "hue {hue}: {}", accent.hex());
        }
    }

    #[test]
    fn dark_custom_colours_take_white_text() {
        assert_eq!(Accent { rgb: (0x20, 0x30, 0x80), custom: true }.text(), Color::White);
    }

    #[test]
    fn hex_parses_with_or_without_the_hash() {
        assert_eq!(parse_hex("#FF8800"), Some((255, 136, 0)));
        assert_eq!(parse_hex("ff8800"), Some((255, 136, 0)));
        assert_eq!(parse_hex("#f80"), None);
        assert_eq!(parse_hex("#gg8800"), None);
        assert_eq!(Accent { rgb: (255, 136, 0), custom: true }.hex(), "#ff8800");
    }

    #[test]
    fn oklch_gives_known_colours() {
        // Zero chroma is grey, and full lightness white.
        assert_eq!(oklch_to_rgb(1.0, 0.0, 0.0), (255, 255, 255));
        let (r, g, b) = oklch_to_rgb(0.6, 0.0, 0.0);
        assert!(r == g && g == b);
    }
}
