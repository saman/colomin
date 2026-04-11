#![allow(dead_code)]

use gpui::{rgb, Hsla};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// The resolved colors used throughout the app
#[derive(Debug, Clone)]
pub struct ThemeColors {
    pub bg: Hsla,
    pub surface: Hsla,
    pub border: Hsla,
    pub text_primary: Hsla,
    pub text_secondary: Hsla,
    pub text_tertiary: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_text: Hsla,
    pub accent_subtle: Hsla,
    pub edited: Hsla,
    pub hover_row: Hsla,
    pub danger: Hsla,
    pub status_bar_bg: Hsla,
    pub gutter_bg: Hsla,
    pub line_number: Hsla,
    pub selection: Hsla,
}

/// A loaded Zed theme with a name and resolved colors
#[derive(Debug, Clone)]
pub struct ZedTheme {
    pub name: String,
    pub appearance: ThemeAppearance,
    pub colors: ThemeColors,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThemeAppearance {
    Light,
    Dark,
}

// ── Zed theme JSON schema ──

#[derive(Deserialize)]
struct ZedThemeFile {
    name: Option<String>,
    themes: Vec<ZedThemeEntry>,
}

#[derive(Deserialize)]
struct ZedThemeEntry {
    name: String,
    appearance: String,
    style: ZedStyle,
}

#[derive(Deserialize)]
struct ZedStyle {
    #[serde(flatten)]
    tokens: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct ZedPlayer {
    cursor: Option<String>,
    selection: Option<String>,
}

// ── Color parsing ──

fn parse_hex_color(s: &str) -> Option<Hsla> {
    let s = s.trim_start_matches('#');
    let (r, g, b, a) = match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            (r, g, b, 255u8)
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            (r, g, b, a)
        }
        _ => return None,
    };
    Some(rgba_to_hsla(r, g, b, a as f32 / 255.0))
}

fn rgba_to_hsla(r: u8, g: u8, b: u8, a: f32) -> Hsla {
    // Use gpui's rgb conversion for accuracy
    let rgba = gpui::rgba(
        ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a * 255.0) as u32,
    );
    rgba.into()
}

fn hex(color: u32) -> Hsla {
    rgb(color).into()
}

// ── Token extraction ──

impl ZedStyle {
    fn get_color(&self, key: &str) -> Option<Hsla> {
        self.tokens
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(parse_hex_color)
    }

    fn get_color_or(&self, key: &str, fallback: Hsla) -> Hsla {
        self.get_color(key).unwrap_or(fallback)
    }

    fn get_players(&self) -> Vec<ZedPlayer> {
        self.tokens
            .get("players")
            .and_then(|v| serde_json::from_value::<Vec<ZedPlayer>>(v.clone()).ok())
            .unwrap_or_default()
    }
}

// ── Theme conversion ──

fn zed_style_to_colors(style: &ZedStyle, appearance: ThemeAppearance) -> ThemeColors {
    let players = style.get_players();
    let accent = players
        .first()
        .and_then(|p| p.cursor.as_ref())
        .and_then(|c| parse_hex_color(c))
        .unwrap_or_else(|| match appearance {
            ThemeAppearance::Light => hex(0x3B82F6),
            ThemeAppearance::Dark => hex(0x60A5FA),
        });

    let selection = players
        .first()
        .and_then(|p| p.selection.as_ref())
        .and_then(|c| parse_hex_color(c))
        .unwrap_or_else(|| accent.opacity(0.15));

    let bg = style.get_color_or(
        "background",
        match appearance {
            ThemeAppearance::Light => hex(0xFFFFFF),
            ThemeAppearance::Dark => hex(0x1A1A1A),
        },
    );

    let surface = style
        .get_color("editor.background")
        .or_else(|| style.get_color("surface.background"))
        .unwrap_or(bg);

    let border = style.get_color_or(
        "border",
        match appearance {
            ThemeAppearance::Light => hex(0xE0E0E0),
            ThemeAppearance::Dark => hex(0x3A3A3A),
        },
    );

    let text_primary = style
        .get_color("editor.foreground")
        .or_else(|| style.get_color("text"))
        .unwrap_or_else(|| match appearance {
            ThemeAppearance::Light => hex(0x000000),
            ThemeAppearance::Dark => hex(0xDDDDDD),
        });

    let text_secondary = style.get_color_or(
        "text.muted",
        match appearance {
            ThemeAppearance::Light => hex(0x555555),
            ThemeAppearance::Dark => hex(0x9E9E9E),
        },
    );

    let text_tertiary = style.get_color_or(
        "text.placeholder",
        match appearance {
            ThemeAppearance::Light => hex(0x929292),
            ThemeAppearance::Dark => hex(0x6E6E6E),
        },
    );

    let hover_row = style
        .get_color("editor.active_line.background")
        .or_else(|| style.get_color("ghost_element.hover"))
        .unwrap_or_else(|| match appearance {
            ThemeAppearance::Light => hex(0xF0F0F0),
            ThemeAppearance::Dark => hex(0x272727),
        });

    let status_bar_bg = style.get_color("status_bar.background").unwrap_or(bg);

    let gutter_bg = style
        .get_color("editor.gutter.background")
        .unwrap_or(surface);

    let line_number = style.get_color_or("editor.line_number", text_tertiary);

    let edited = style
        .get_color("modified.background")
        .unwrap_or_else(|| match appearance {
            ThemeAppearance::Light => hex(0xFFF2E5),
            ThemeAppearance::Dark => hex(0x3A310E),
        });

    let danger = style.get_color_or(
        "error",
        match appearance {
            ThemeAppearance::Light => hex(0xEF4444),
            ThemeAppearance::Dark => hex(0xEF4444),
        },
    );

    let accent_subtle = selection;

    ThemeColors {
        bg,
        surface,
        border,
        text_primary,
        text_secondary,
        text_tertiary,
        accent,
        accent_hover: accent,
        accent_text: if appearance == ThemeAppearance::Light {
            hex(0xFFFFFF)
        } else {
            hex(0x000000)
        },
        accent_subtle,
        edited,
        hover_row,
        danger,
        status_bar_bg,
        gutter_bg,
        line_number,
        selection,
    }
}

// ── Public API ──

/// Parse a Zed theme JSON string into a list of themes
#[must_use]
pub fn parse_zed_theme(json: &str) -> Result<Vec<ZedTheme>, String> {
    let file: ZedThemeFile =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse theme: {}", e))?;

    let mut themes = Vec::new();
    for entry in &file.themes {
        let appearance = match entry.appearance.as_str() {
            "light" => ThemeAppearance::Light,
            "dark" => ThemeAppearance::Dark,
            _ => ThemeAppearance::Dark,
        };

        let colors = zed_style_to_colors(&entry.style, appearance);
        themes.push(ZedTheme {
            name: entry.name.clone(),
            appearance,
            colors,
        });
    }

    Ok(themes)
}

/// Load a Zed theme from a file path
#[must_use]
#[allow(dead_code)]
pub fn load_zed_theme_file(path: &Path) -> Result<Vec<ZedTheme>, String> {
    let json =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read theme file: {}", e))?;
    parse_zed_theme(&json)
}

/// Get all bundled themes
pub fn bundled_themes() -> Vec<ZedTheme> {
    let mut all = Vec::new();

    // Colomin Light (our original design) — always first / default
    all.push(ZedTheme {
        name: "Colomin Light".into(),
        appearance: ThemeAppearance::Light,
        colors: default_light(),
    });

    // macOS Classic
    let macos_json = include_str!("../../themes/macos-classic.json");
    if let Ok(themes) = parse_zed_theme(macos_json) {
        all.extend(themes);
    }

    // GitHub Theme
    let github_json = include_str!("../../themes/github.json");
    if let Ok(themes) = parse_zed_theme(github_json) {
        all.extend(themes);
    }

    // Fallback built-in themes if JSON parsing fails
    if all.is_empty() {
        all.push(ZedTheme {
            name: "Light".into(),
            appearance: ThemeAppearance::Light,
            colors: default_light(),
        });
        all.push(ZedTheme {
            name: "Dark".into(),
            appearance: ThemeAppearance::Dark,
            colors: default_dark(),
        });
    }

    all
}

pub fn default_light() -> ThemeColors {
    ThemeColors {
        bg: hex(0xFAFAFA),
        surface: hex(0xFFFFFF),
        border: hex(0xEEEEEE),
        text_primary: hex(0x1A1A1A),
        text_secondary: hex(0x8A8A8A),
        text_tertiary: hex(0xB0B0B0),
        accent: hex(0x3B82F6),
        accent_hover: hex(0x2563EB),
        accent_text: hex(0xFFFFFF),
        accent_subtle: hex(0xEFF6FF),
        edited: hex(0xFFF7ED),
        hover_row: hex(0xF5F8FF),
        danger: hex(0xEF4444),
        status_bar_bg: hex(0xFAFAFA),
        gutter_bg: hex(0xFFFFFF),
        line_number: hex(0xB0B0B0),
        selection: hex(0xC7DEFF),
    }
}

pub fn default_dark() -> ThemeColors {
    ThemeColors {
        bg: hex(0x0F0F0F),
        surface: hex(0x1A1A1A),
        border: hex(0x252525),
        text_primary: hex(0xE5E5E5),
        text_secondary: hex(0x6B6B6B),
        text_tertiary: hex(0x4A4A4A),
        accent: hex(0x60A5FA),
        accent_hover: hex(0x3B82F6),
        accent_text: hex(0x000000),
        accent_subtle: hex(0x172554),
        edited: hex(0x332200),
        hover_row: hex(0x111827),
        danger: hex(0xEF4444),
        status_bar_bg: hex(0x0F0F0F),
        gutter_bg: hex(0x1A1A1A),
        line_number: hex(0x4A4A4A),
        selection: hex(0x172554),
    }
}
