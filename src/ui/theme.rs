#![allow(dead_code)]

use egui::Color32;
use serde::Deserialize;
use std::path::Path;

/// The resolved colors used throughout the app.
#[derive(Debug, Clone)]
pub struct ThemeColors {
    pub bg: Color32,
    pub surface: Color32,
    pub border: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_text: Color32,
    pub accent_subtle: Color32,
    pub edited: Color32,
    pub hover_row: Color32,
    pub danger: Color32,
    pub status_bar_bg: Color32,
    pub gutter_bg: Color32,
    pub line_number: Color32,
    pub selection: Color32,
}

/// A loaded theme with a name and resolved colors.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub appearance: ThemeAppearance,
    pub colors: ThemeColors,
}

// Keep the old type alias so existing code doesn't break during migration.
pub type ZedTheme = Theme;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ThemeAppearance {
    Light,
    Dark,
}

// ── DTCG Design Token format (W3C Design Tokens Community Group) ──

/// A DTCG color token value.
/// Supports the full object form: { colorSpace, components, alpha?, hex? }
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum DtcgColorValue {
    /// Full object form per the DTCG spec.
    Object {
        #[serde(rename = "colorSpace")]
        color_space: Option<String>,
        components: Vec<f64>,
        alpha: Option<f64>,
        hex: Option<String>,
    },
    /// Legacy shorthand: plain hex string (e.g. "#FF0000").
    HexString(String),
}

/// A single design token with $value (and optional $type, $description).
#[derive(Deserialize, Debug)]
struct DtcgToken {
    #[serde(rename = "$value")]
    value: serde_json::Value,
    #[serde(rename = "$type")]
    _type: Option<String>,
    #[serde(rename = "$description")]
    _description: Option<String>,
}

// ── Color helpers ──

fn hex(color: u32) -> Color32 {
    let r = ((color >> 16) & 0xFF) as u8;
    let g = ((color >> 8) & 0xFF) as u8;
    let b = (color & 0xFF) as u8;
    Color32::from_rgb(r, g, b)
}

fn parse_hex_color(s: &str) -> Option<Color32> {
    let s = s.trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some(Color32::from_rgb(r, g, b))
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some(Color32::from_rgba_unmultiplied(r, g, b, a))
        }
        _ => None,
    }
}

/// Convert a DTCG color value (object or hex string) to an egui Color32.
fn dtcg_color_to_color32(val: &serde_json::Value) -> Option<Color32> {
    let parsed: DtcgColorValue = serde_json::from_value(val.clone()).ok()?;
    match parsed {
        DtcgColorValue::Object { components, alpha, hex: hex_str, .. } => {
            if components.len() >= 3 {
                let r = (components[0].clamp(0.0, 1.0) * 255.0).round() as u8;
                let g = (components[1].clamp(0.0, 1.0) * 255.0).round() as u8;
                let b = (components[2].clamp(0.0, 1.0) * 255.0).round() as u8;
                let a = alpha.map(|a| (a.clamp(0.0, 1.0) * 255.0).round() as u8).unwrap_or(255);
                Some(Color32::from_rgba_unmultiplied(r, g, b, a))
            } else {
                // Fallback to hex if components are incomplete
                hex_str.as_deref().and_then(parse_hex_color)
            }
        }
        DtcgColorValue::HexString(s) => parse_hex_color(&s),
    }
}

// ── DTCG Token file parsing ──

/// Navigate into a JSON object by a dot-separated path.
fn json_get<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for key in path.split('.') {
        current = current.get(key)?;
    }
    Some(current)
}

/// Extract a Color32 from a DTCG token file at the given path.
/// Path uses dots to navigate groups, ending at a token with $value.
fn resolve_color(root: &serde_json::Value, path: &str) -> Option<Color32> {
    let token_obj = json_get(root, path)?;
    let value = token_obj.get("$value")?;
    dtcg_color_to_color32(value)
}

/// Parse a DTCG `.tokens.json` file into a Theme.
/// Expects the standard Colomin token structure with groups:
///   color.background, color.surface, color.border,
///   color.text.{primary,secondary,tertiary},
///   color.accent.{default,hover,on-accent,subtle},
///   color.state.{edited,hover-row,danger,selection},
///   color.chrome.{status-bar,gutter,line-number}
fn parse_dtcg_theme(json: &str, name: &str) -> Result<Theme, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Failed to parse theme '{}': {}", name, e))?;

    // Determine appearance from $extensions.dev.colomin.appearance
    let appearance = root
        .get("$extensions")
        .and_then(|ext| ext.get("dev.colomin"))
        .and_then(|cm| cm.get("appearance"))
        .and_then(|a| a.as_str())
        .map(|s| match s {
            "light" => ThemeAppearance::Light,
            _ => ThemeAppearance::Dark,
        })
        .unwrap_or(ThemeAppearance::Dark);

    // Fallback defaults based on appearance
    let (fb_bg, fb_surface, fb_border) = match appearance {
        ThemeAppearance::Light => (hex(0xFFFFFF), hex(0xFFFFFF), hex(0xE0E0E0)),
        ThemeAppearance::Dark => (hex(0x1A1A1A), hex(0x1A1A1A), hex(0x3A3A3A)),
    };
    let (fb_text_pri, fb_text_sec, fb_text_ter) = match appearance {
        ThemeAppearance::Light => (hex(0x000000), hex(0x555555), hex(0x929292)),
        ThemeAppearance::Dark => (hex(0xDDDDDD), hex(0x9E9E9E), hex(0x6E6E6E)),
    };
    let (fb_accent, fb_danger) = match appearance {
        ThemeAppearance::Light => (hex(0x3B82F6), hex(0xEF4444)),
        ThemeAppearance::Dark => (hex(0x60A5FA), hex(0xEF4444)),
    };

    let bg = resolve_color(&root, "color.background").unwrap_or(fb_bg);
    let surface = resolve_color(&root, "color.surface").unwrap_or(fb_surface);
    let border = resolve_color(&root, "color.border").unwrap_or(fb_border);
    let text_primary = resolve_color(&root, "color.text.primary").unwrap_or(fb_text_pri);
    let text_secondary = resolve_color(&root, "color.text.secondary").unwrap_or(fb_text_sec);
    let text_tertiary = resolve_color(&root, "color.text.tertiary").unwrap_or(fb_text_ter);
    let accent = resolve_color(&root, "color.accent.default").unwrap_or(fb_accent);
    let accent_hover = resolve_color(&root, "color.accent.hover").unwrap_or(accent);
    let accent_text = resolve_color(&root, "color.accent.on-accent")
        .unwrap_or(if appearance == ThemeAppearance::Light { hex(0xFFFFFF) } else { hex(0x000000) });
    let accent_subtle = resolve_color(&root, "color.accent.subtle").unwrap_or(accent);
    let edited = resolve_color(&root, "color.state.edited").unwrap_or(bg);
    let hover_row = resolve_color(&root, "color.state.hover-row").unwrap_or(bg);
    let danger = resolve_color(&root, "color.state.danger").unwrap_or(fb_danger);
    let selection = resolve_color(&root, "color.state.selection").unwrap_or(accent_subtle);
    let status_bar_bg = resolve_color(&root, "color.chrome.status-bar").unwrap_or(bg);
    let gutter_bg = resolve_color(&root, "color.chrome.gutter").unwrap_or(surface);
    let line_number = resolve_color(&root, "color.chrome.line-number").unwrap_or(text_tertiary);

    Ok(Theme {
        name: name.to_string(),
        appearance,
        colors: ThemeColors {
            bg,
            surface,
            border,
            text_primary,
            text_secondary,
            text_tertiary,
            accent,
            accent_hover,
            accent_text,
            accent_subtle,
            edited,
            hover_row,
            danger,
            status_bar_bg,
            gutter_bg,
            line_number,
            selection,
        },
    })
}

/// Apply a ThemeColors palette to an egui Context.
pub fn apply_theme(ctx: &egui::Context, colors: &ThemeColors) {
    let mut visuals = egui::Visuals::light();

    visuals.panel_fill = colors.bg;
    visuals.window_fill = colors.surface;
    visuals.extreme_bg_color = colors.surface;
    visuals.faint_bg_color = colors.surface;

    visuals.widgets.noninteractive.bg_fill = colors.surface;
    visuals.widgets.noninteractive.weak_bg_fill = colors.surface;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, colors.border);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, colors.text_primary);

    visuals.widgets.inactive.bg_fill = colors.surface;
    visuals.widgets.inactive.weak_bg_fill = colors.surface;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, colors.border);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, colors.text_primary);

    visuals.widgets.hovered.bg_fill = colors.hover_row;
    visuals.widgets.hovered.weak_bg_fill = colors.hover_row;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, colors.accent);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, colors.text_primary);

    visuals.widgets.active.bg_fill = colors.accent_subtle;
    visuals.widgets.active.weak_bg_fill = colors.accent_subtle;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, colors.accent);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, colors.text_primary);

    visuals.selection.bg_fill = colors.selection;
    visuals.selection.stroke = egui::Stroke::new(1.0, colors.accent);

    visuals.override_text_color = Some(colors.text_primary);

    ctx.set_visuals(visuals);
}

// ── Public API ──

/// Parse a DTCG theme JSON string.
#[must_use]
pub fn parse_theme(json: &str, name: &str) -> Result<Theme, String> {
    parse_dtcg_theme(json, name)
}

/// Load a DTCG theme from a file path.
#[must_use]
#[allow(dead_code)]
pub fn load_theme_file(path: &Path) -> Result<Theme, String> {
    let json =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read theme file: {}", e))?;
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown");
    // Strip .tokens suffix if present (e.g. "colomin-light.tokens.json" → "colomin-light")
    let name = name.strip_suffix(".tokens").unwrap_or(name);
    parse_dtcg_theme(&json, name)
}

/// Get all bundled themes (loaded from DTCG .tokens.json files at compile time).
pub fn bundled_themes() -> Vec<Theme> {
    let mut all = Vec::new();

    // Colomin Light — always first / default
    let colomin_light_json = include_str!("../../themes/colomin-light.tokens.json");
    if let Ok(theme) = parse_dtcg_theme(colomin_light_json, "Colomin Light") {
        all.push(theme);
    }

    // Colomin Dark
    let colomin_dark_json = include_str!("../../themes/colomin-dark.tokens.json");
    if let Ok(theme) = parse_dtcg_theme(colomin_dark_json, "Colomin Dark") {
        all.push(theme);
    }

    // GitHub Light
    let github_light_json = include_str!("../../themes/github-light.tokens.json");
    if let Ok(theme) = parse_dtcg_theme(github_light_json, "GitHub Light") {
        all.push(theme);
    }

    // GitHub Dark
    let github_dark_json = include_str!("../../themes/github-dark.tokens.json");
    if let Ok(theme) = parse_dtcg_theme(github_dark_json, "GitHub Dark") {
        all.push(theme);
    }

    // Absolute fallback
    if all.is_empty() {
        all.push(Theme {
            name: "Light".into(),
            appearance: ThemeAppearance::Light,
            colors: default_light(),
        });
        all.push(Theme {
            name: "Dark".into(),
            appearance: ThemeAppearance::Dark,
            colors: default_dark(),
        });
    }

    all
}

/// Hardcoded fallback light theme (only used if all JSON parsing fails).
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

/// Hardcoded fallback dark theme (only used if all JSON parsing fails).
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
