// SPDX-License-Identifier: LGPL-3.0-or-later
// SPDX-FileCopyrightText: © GSI Helmholtzzentrum f. Schwerionenforschung GmbH, Darmstadt, Germany

//! The theme plug-in contract (R19): semantic style slots and glyphs that the renderer reads,
//! and the compiled-in table of themes (§9.5). A new theme is one entry in `THEMES`.

use ratatui::style::{Color, Modifier, Style};

/// Semantic style slots: the renderer reads styles only from here, never picks colours itself.
#[derive(Debug)]
pub struct Theme {
    pub name: &'static str,
    pub base: Style,
    pub header: Style,
    pub dim: Style,
    pub accent: Style,
    pub selected: Style,
    pub marked: Style,
    /// Stale nodes, drops.
    pub warn: Style,
    /// Errors, OOM kills.
    pub crit: Style,
    /// Fold glyphs and tree guides.
    pub tree: Style,
    /// Value bands: idle, low, high, saturated.
    pub heat: [Color; 4],
    pub glyphs: Glyphs,
}

/// Glyphs, so ASCII-only themes need no renderer change.
#[derive(Debug)]
pub struct Glyphs {
    pub open: &'static str,
    pub closed: &'static str,
    /// Process tree guides: middle child, last child.
    pub guide: [&'static str; 2],
    /// Bar levels from empty (no or zero samples) to full, for history bars and the heat strip.
    pub bars: [&'static str; 9],
}

/// Unicode tree and bar glyphs shared by most themes.
const UNI: Glyphs = Glyphs {
    open: "▾ ",
    closed: "▸ ",
    guide: ["├ ", "└ "],
    bars: ["·", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"],
};

/// Shorthand for a 24-bit colour, keeps the table readable.
#[allow(clippy::cast_possible_truncation, reason = "each byte of 0xRRGGBB is cut out on purpose")]
const fn rgb(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Builds a coloured theme from its palette: background, panel, selection, text, dim, accent,
/// marked, and the four heat bands (which also provide warn and crit).
const fn palette(
    name: &'static str,
    [bg, panel, sel, text, dim, accent, mark]: [u32; 7],
    heat: [u32; 4],
    glyphs: Glyphs,
) -> Theme {
    let base = Style::new().fg(rgb(text)).bg(rgb(bg));
    let bold = Modifier::BOLD;
    Theme {
        name,
        base,
        header: Style::new().fg(rgb(accent)).bg(rgb(panel)).add_modifier(bold),
        dim: Style::new().fg(rgb(dim)),
        accent: Style::new().fg(rgb(accent)).add_modifier(bold),
        selected: Style::new().bg(rgb(sel)),
        marked: Style::new().fg(rgb(mark)).add_modifier(bold),
        warn: Style::new().fg(rgb(heat[2])),
        crit: Style::new().fg(rgb(heat[3])).add_modifier(bold),
        tree: Style::new().fg(rgb(accent)),
        heat: [rgb(heat[0]), rgb(heat[1]), rgb(heat[2]), rgb(heat[3])],
        glyphs,
    }
}

// budget: data
/// Every theme; the first is the default. `mono` must exist: `NO_COLOR` forces it.
#[allow(clippy::unreadable_literal, reason = "0xRRGGBB reads like the palettes it comes from")]
pub const THEMES: &[Theme] = &[
    // Nord Frost (owner's choice): nord0/1/2 panels, nord4 text, frost nord8, aurora heat.
    palette(
        "nord",
        [0x2E3440, 0x3B4252, 0x434C5E, 0xD8DEE9, 0x7B88A1, 0x88C0D0, 0xEBCB8B],
        [0xA3BE8C, 0xEBCB8B, 0xD08770, 0xBF616A],
        UNI,
    ),
    palette(
        "night",
        [0x11121A, 0x1F2335, 0x283457, 0xC0CAF5, 0x565F89, 0x7AA2F7, 0xE0AF68],
        [0x9ECE6A, 0xE0AF68, 0xFF9E64, 0xF7768E],
        UNI,
    ),
    palette(
        "mocha",
        [0x1E1E2E, 0x181825, 0x313244, 0xCDD6F4, 0x7F849C, 0xCBA6F7, 0xF9E2AF],
        [0xA6E3A1, 0xF9E2AF, 0xFAB387, 0xF38BA8],
        UNI,
    ),
    palette(
        "gruvbox",
        [0x282828, 0x504945, 0x3C3836, 0xEBDBB2, 0x928374, 0xFABD2F, 0x83A598],
        [0xB8BB26, 0xFABD2F, 0xFE8019, 0xFB4934],
        UNI,
    ),
    palette(
        "solar",
        [0xFDF6E3, 0xEEE8D5, 0xEEE8D5, 0x657B83, 0x93A1A1, 0x268BD2, 0xD33682],
        [0x859900, 0xB58900, 0xCB4B16, 0xDC322F],
        UNI,
    ),
    // Amber phosphor: one hue, an ASCII ramp like a 1980s terminal.
    palette(
        "amber",
        [0x140C00, 0x2A1C00, 0x3A2700, 0xFFB000, 0x8A5F00, 0xFFC233, 0xFFE08A],
        [0x8A5F00, 0xC48800, 0xFFB000, 0xFFE08A],
        Glyphs { bars: [".", "_", "-", "=", "+", "*", "#", "%", "@"], ..UNI },
    ),
    Theme {
        name: "mono",
        base: Style::new(),
        header: Style::new().add_modifier(Modifier::REVERSED.union(Modifier::BOLD)),
        dim: Style::new().add_modifier(Modifier::DIM),
        accent: Style::new().add_modifier(Modifier::BOLD),
        selected: Style::new().add_modifier(Modifier::REVERSED),
        marked: Style::new().add_modifier(Modifier::UNDERLINED),
        warn: Style::new().add_modifier(Modifier::BOLD),
        crit: Style::new().add_modifier(Modifier::BOLD.union(Modifier::UNDERLINED)),
        tree: Style::new(),
        heat: [Color::Reset; 4],
        glyphs: UNI,
    },
];
// budget: end

/// Resolves the theme: `--theme`, then `UNJAM_THEME`, then the first; `NO_COLOR` forces `mono`.
pub fn pick(flag: Option<&str>, env: Option<&str>, no_color: bool) -> Result<usize, String> {
    let name = if no_color { Some("mono") } else { flag.or(env) };
    let Some(name) = name else { return Ok(0) };
    let src = if flag.is_none() && !no_color { "UNJAM_THEME: " } else { "" };
    THEMES.iter().position(|t| t.name == name).ok_or_else(|| {
        let names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
        format!("{src}unknown theme {name:?} (themes: {})", names.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_order() {
        assert_eq!(THEMES[0].name, "nord");
        let mono = THEMES.iter().position(|t| t.name == "mono").unwrap();
        assert_eq!(pick(None, None, false), Ok(0));
        assert_eq!(pick(None, Some("gruvbox"), false), Ok(3));
        assert_eq!(pick(Some("solar"), Some("gruvbox"), false), Ok(4));
        assert_eq!(pick(Some("solar"), Some("gruvbox"), true), Ok(mono));
        assert!(pick(Some("nope"), None, false).unwrap_err().contains("nord, night"));
        assert!(pick(None, Some("nope"), false).unwrap_err().starts_with("UNJAM_THEME: unknown"));
        let names: Vec<_> = THEMES.iter().map(|t| t.name).collect();
        assert_eq!(names, ["nord", "night", "mocha", "gruvbox", "solar", "amber", "mono"]);
    }

    #[test]
    fn nord_matches_the_palette() {
        let t = &THEMES[0];
        assert_eq!(t.base.bg, Some(Color::Rgb(0x2E, 0x34, 0x40)));
        assert_eq!(t.heat[3], Color::Rgb(0xBF, 0x61, 0x6A));
    }
}
