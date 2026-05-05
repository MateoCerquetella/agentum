//! The single, canonical palette for the TUI dashboard.
//!
//! See `docs/DESIGN-SYSTEM.md` for the design rationale. The previous
//! multi-theme registry (`system` / `midnight` / `dusk` / `slate` /
//! `paper` / `mono`) has been retired in favour of one disciplined dark
//! palette. `~/.local/share/agentum/theme` and `$AGENTUM_THEME` are
//! still read for back-compat but their value is ignored — there is
//! only the canonical theme.

use std::path::PathBuf;

use ratatui::style::Color;

/// Concrete palette consumed by `ui.rs`. Every visible color goes through
/// here, including the panel backgrounds.
#[derive(Clone, Copy)]
pub struct Palette {
    // Layered backgrounds. `body_bg` paints the void around panels;
    // `panel_bg` is the inside of each pane block; `surface_bg` is for
    // raised UI like the input bar and cursor row; `chrome_bg` is the
    // status bar.
    pub body_bg: Color,
    pub panel_bg: Color,
    pub surface_bg: Color,
    pub chrome_bg: Color,

    pub fg: Color,
    pub fg_strong: Color,
    pub muted: Color,
    pub subtle: Color,

    pub accent: Color,
    pub accent_alt: Color,
    pub idle_border: Color,
    pub focus_border: Color,

    pub cursor_bg: Color,
    pub cursor_fg: Color,

    pub success: Color,
    pub warning: Color,
    pub error: Color,

    pub chip_bg: Color,
    pub chip_fg: Color,
}

pub struct Theme {
    pub palette: Palette,
}

// ---------- the only palette ----------
//
// Near-black canvas, pure achromatic gray ramp, electric blue for
// activation, coral-red for warm punctuation, neon green for success.
// See `docs/DESIGN-SYSTEM.md` § 2 for role mapping.

const PALETTE: Theme = Theme {
    palette: Palette {
        // Surfaces — colorimetric depth, no shadows.
        body_bg: Color::Rgb(0x0b, 0x0b, 0x0b),    // #0b0b0b near-black canvas
        panel_bg: Color::Rgb(0x21, 0x21, 0x21),   // #212121 elevated surface
        surface_bg: Color::Rgb(0x35, 0x35, 0x35), // #353535 medium dark
        chrome_bg: Color::Rgb(0x00, 0x00, 0x00),  // pure black for status bar

        // Text ramp.
        fg: Color::Rgb(0xb9, 0xb9, 0xb9),         // #b9b9b9 silver (body)
        fg_strong: Color::Rgb(0xff, 0xff, 0xff),  // white (titles)
        muted: Color::Rgb(0x79, 0x79, 0x79),      // #797979 metadata
        subtle: Color::Rgb(0x35, 0x35, 0x35),     // #353535 dimmed

        // Interactive — electric blue is the universal activation signal,
        // coral-red is the warm CTA punctuation.
        accent: Color::Rgb(0x00, 0x52, 0xef),     // #0052ef electric blue
        accent_alt: Color::Rgb(0xf3, 0x64, 0x58), // #f36458 coral CTA
        idle_border: Color::Rgb(0x21, 0x21, 0x21),
        focus_border: Color::Rgb(0x00, 0x52, 0xef),

        // Cursor row — slightly brighter surface, white glyphs.
        cursor_bg: Color::Rgb(0x35, 0x35, 0x35),
        cursor_fg: Color::Rgb(0xff, 0xff, 0xff),

        // Semantic.
        success: Color::Rgb(0x19, 0xd6, 0x00),    // #19d600 neon green sRGB
        warning: Color::Rgb(0xf3, 0x64, 0x58),    // #f36458 coral
        error: Color::Rgb(0xdd, 0x00, 0x00),      // #dd0000 pure red

        // Chips / pills.
        chip_bg: Color::Rgb(0x21, 0x21, 0x21),
        chip_fg: Color::Rgb(0xb9, 0xb9, 0xb9),
    },
};

// ---------- persistence ----------
//
// Kept so a stale `~/.local/share/agentum/theme` file or `$AGENTUM_THEME`
// export doesn't crash anything — we just always return the canonical
// palette.

fn theme_file() -> Option<PathBuf> {
    let dir = agentum_store::paths::data_dir().ok()?;
    Some(dir.join("theme"))
}

pub fn load() -> &'static Theme {
    // Read & ignore — only one palette exists.
    let _ = std::env::var("AGENTUM_THEME");
    if let Some(path) = theme_file() {
        let _ = std::fs::read_to_string(&path);
    }
    &PALETTE
}
