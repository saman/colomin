//! SVG icon helpers.
//!
//! Embeds each icon as static bytes at compile-time.
//! Call `egui_extras::install_image_loaders(ctx)` once at startup so the SVG
//! loader is registered before any icons are drawn.
//!
//! All SVG files use `stroke="white"` so that `.tint(color)` works correctly:
//! a white pixel × tint_color = tint_color.

use eframe::egui;

/// Return a 15×15 `egui::Image` for the named icon, tinted with `color`.
pub fn icon(name: &str, color: egui::Color32) -> egui::Image<'static> {
    egui::Image::new(source(name))
        .fit_to_exact_size(egui::vec2(15.0, 15.0))
        .tint(color)
}

/// Build an `ImageSource` backed by embedded bytes for the named icon.
pub fn source(name: &str) -> egui::ImageSource<'static> {
    macro_rules! entry {
        ($tag:expr, $file:expr) => {
            (
                $tag,
                include_bytes!(concat!("../../assets/icons/", $file)).as_slice(),
            )
        };
    }

    let pair: (&'static str, &'static [u8]) = match name {
        "columns-on"           => entry!("bytes://columns-on.svg",           "columns-on.svg"),
        "columns-off"          => entry!("bytes://columns-off.svg",          "columns-off.svg"),
        "theme"                => entry!("bytes://theme.svg",                "theme.svg"),
        "gear"                 => entry!("bytes://gear.svg",                  "gear.svg"),
        "settings"             => entry!("bytes://settings.svg",             "settings.svg"),
        "header-toggle"        => entry!("bytes://header-toggle.svg",        "header-toggle.svg"),
        "chevron-right"        => entry!("bytes://chevron-right.svg",        "chevron-right.svg"),
        "chevron-left"         => entry!("bytes://chevron-left.svg",         "chevron-left.svg"),
        "chevron-up"           => entry!("bytes://chevron-up.svg",           "chevron-up.svg"),
        "chevron-down"         => entry!("bytes://chevron-down.svg",         "chevron-down.svg"),
        "stat-count"           => entry!("bytes://stat-count.svg",           "stat-count.svg"),
        "stat-sum"             => entry!("bytes://stat-sum.svg",             "stat-sum.svg"),
        "stat-avg"             => entry!("bytes://stat-avg.svg",             "stat-avg.svg"),
        "stat-min"             => entry!("bytes://stat-min.svg",             "stat-min.svg"),
        "stat-max"             => entry!("bytes://stat-max.svg",             "stat-max.svg"),
        "stat-length"          => entry!("bytes://stat-length.svg",          "stat-length.svg"),
        "modified"             => entry!("bytes://modified.svg",             "modified.svg"),
        "sort-asc"             => entry!("bytes://sort-asc.svg",             "sort-asc.svg"),
        "sort-desc"            => entry!("bytes://sort-desc.svg",            "sort-desc.svg"),
        "copy"                 => entry!("bytes://copy.svg",                 "copy.svg"),
        "paste"                => entry!("bytes://paste.svg",                "paste.svg"),
        "search"               => entry!("bytes://search.svg",               "search.svg"),
        "moon"                 => entry!("bytes://moon.svg",                 "moon.svg"),
        "sun"                  => entry!("bytes://sun.svg",                  "sun.svg"),
        "edit"                 => entry!("bytes://edit.svg",                 "edit.svg"),
        "pen"                  => entry!("bytes://pen.svg",                  "pen.svg"),
        "undo"                 => entry!("bytes://undo.svg",                 "undo.svg"),
        "redo"                 => entry!("bytes://redo.svg",                 "redo.svg"),
        "delete-row"           => entry!("bytes://delete-row.svg",           "delete-row.svg"),
        "delete-column"        => entry!("bytes://delete-column.svg",        "delete-column.svg"),
        "insert-row-above"     => entry!("bytes://insert-row-above.svg",     "insert-row-above.svg"),
        "insert-row-below"     => entry!("bytes://insert-row-below.svg",     "insert-row-below.svg"),
        "insert-column-left"   => entry!("bytes://insert-column-left.svg",   "insert-column-left.svg"),
        "insert-column-right"  => entry!("bytes://insert-column-right.svg",  "insert-column-right.svg"),
        "debug"                => entry!("bytes://debug.svg",                "debug.svg"),
        "font"                 => entry!("bytes://font.svg",                 "font.svg"),
        "tabs"                 => entry!("bytes://tabs.svg",                 "tabs.svg"),
        "row-hover"            => entry!("bytes://row-hover.svg",            "row-hover.svg"),
        "zoom"                 => entry!("bytes://zoom.svg",                 "zoom.svg"),
        "copy-mode"            => entry!("bytes://copy-mode.svg",            "copy-mode.svg"),
        "copy-text"            => entry!("bytes://copy-text.svg",            "copy-text.svg"),
        "copy-csv"             => entry!("bytes://copy-csv.svg",             "copy-csv.svg"),
        "copy-json"            => entry!("bytes://copy-json.svg",            "copy-json.svg"),
        "copy-markdown"        => entry!("bytes://copy-markdown.svg",        "copy-markdown.svg"),
        _                      => ("bytes://empty.svg", &[]),
    };
    pair.into()
}

/// Icon name for PreferredStat variants.
pub fn stat_icon_name(stat: crate::state::PreferredStat) -> &'static str {
    use crate::state::PreferredStat::*;
    match stat {
        Count  => "stat-count",
        Sum    => "stat-sum",
        Avg    => "stat-avg",
        Min    => "stat-min",
        Max    => "stat-max",
        Length => "stat-length",
    }
}
