// ABOUTME: Palette + named themes inspired by sniffnet. A global `OnceLock`
// ABOUTME: holds the active palette so widget-style closures can read it.

use iced::Color;
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// App background.
    pub primary: Color,
    /// Accent color for selection / primary buttons.
    pub secondary: Color,
    /// Panel and toolbar surfaces.
    pub surface: Color,
    /// Hovered / pressed background on interactive surfaces.
    pub elevated: Color,
    /// Body text.
    pub text: Color,
    /// Subdued / label text.
    pub text_dim: Color,
    /// Hairlines and inset borders.
    pub border: Color,
    /// Errors, delete actions.
    pub danger: Color,
    /// Header gradient: from this color...
    pub header_grad_from: Color,
    /// ...to this one.
    pub header_grad_to: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemeKind {
    Dark,
    Light,
    Nord,
    Dracula,
    Gruvbox,
    Solarized,
}

impl Default for ThemeKind {
    fn default() -> Self {
        ThemeKind::Dark
    }
}

impl ThemeKind {
    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Dark => "Dark",
            ThemeKind::Light => "Light",
            ThemeKind::Nord => "Nord",
            ThemeKind::Dracula => "Dracula",
            ThemeKind::Gruvbox => "Gruvbox",
            ThemeKind::Solarized => "Solarized",
        }
    }

    pub fn all() -> &'static [ThemeKind] {
        &[
            ThemeKind::Dark,
            ThemeKind::Light,
            ThemeKind::Nord,
            ThemeKind::Dracula,
            ThemeKind::Gruvbox,
            ThemeKind::Solarized,
        ]
    }

    pub fn palette(self) -> Palette {
        match self {
            ThemeKind::Dark => palette_dark(),
            ThemeKind::Light => palette_light(),
            ThemeKind::Nord => palette_nord(),
            ThemeKind::Dracula => palette_dracula(),
            ThemeKind::Gruvbox => palette_gruvbox(),
            ThemeKind::Solarized => palette_solarized(),
        }
    }
}

impl std::fmt::Display for ThemeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// --- Global active palette ---------------------------------------------------

static ACTIVE: OnceLock<RwLock<Palette>> = OnceLock::new();

pub fn set_active(p: Palette) {
    let cell = ACTIVE.get_or_init(|| RwLock::new(palette_dark()));
    if let Ok(mut g) = cell.write() {
        *g = p;
    }
}

pub fn active() -> Palette {
    ACTIVE
        .get_or_init(|| RwLock::new(palette_dark()))
        .read()
        .map(|g| *g)
        .unwrap_or_else(|_| palette_dark())
}

// --- Built-in palettes -------------------------------------------------------

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

fn palette_dark() -> Palette {
    Palette {
        primary: rgb(0x15, 0x18, 0x1f),
        secondary: rgb(0x5e, 0xa0, 0xff),
        surface: rgb(0x1f, 0x24, 0x2e),
        elevated: rgb(0x2a, 0x31, 0x3e),
        text: rgb(0xe5, 0xe9, 0xf0),
        text_dim: rgb(0x8b, 0x96, 0xa8),
        border: rgb(0x3b, 0x44, 0x55),
        danger: rgb(0xff, 0x6e, 0x6e),
        header_grad_from: rgb(0x29, 0x4c, 0x7a),
        header_grad_to: rgb(0x5e, 0xa0, 0xff),
    }
}

fn palette_light() -> Palette {
    Palette {
        primary: rgb(0xf6, 0xf7, 0xfa),
        secondary: rgb(0x3b, 0x7b, 0xd9),
        surface: rgb(0xff, 0xff, 0xff),
        elevated: rgb(0xed, 0xf1, 0xf7),
        text: rgb(0x1c, 0x22, 0x2e),
        text_dim: rgb(0x6a, 0x73, 0x82),
        border: rgb(0xd4, 0xdb, 0xe5),
        danger: rgb(0xd0, 0x42, 0x3e),
        header_grad_from: rgb(0x3b, 0x7b, 0xd9),
        header_grad_to: rgb(0x8e, 0xb7, 0xee),
    }
}

fn palette_nord() -> Palette {
    Palette {
        primary: rgb(0x2e, 0x34, 0x40),       // nord0
        secondary: rgb(0x88, 0xc0, 0xd0),     // nord8
        surface: rgb(0x3b, 0x42, 0x52),       // nord1
        elevated: rgb(0x43, 0x4c, 0x5e),      // nord2
        text: rgb(0xec, 0xef, 0xf4),          // nord6
        text_dim: rgb(0xd8, 0xde, 0xe9),      // nord4
        border: rgb(0x4c, 0x56, 0x6a),        // nord3
        danger: rgb(0xbf, 0x61, 0x6a),        // nord11
        header_grad_from: rgb(0x5e, 0x81, 0xac), // nord10
        header_grad_to: rgb(0x88, 0xc0, 0xd0),   // nord8
    }
}

fn palette_dracula() -> Palette {
    Palette {
        primary: rgb(0x28, 0x2a, 0x36),
        secondary: rgb(0xbd, 0x93, 0xf9),
        surface: rgb(0x32, 0x35, 0x44),
        elevated: rgb(0x44, 0x47, 0x5a),
        text: rgb(0xf8, 0xf8, 0xf2),
        text_dim: rgb(0x6d, 0x77, 0x97),
        border: rgb(0x44, 0x47, 0x5a),
        danger: rgb(0xff, 0x55, 0x55),
        header_grad_from: rgb(0xff, 0x79, 0xc6),
        header_grad_to: rgb(0xbd, 0x93, 0xf9),
    }
}

fn palette_gruvbox() -> Palette {
    Palette {
        primary: rgb(0x28, 0x28, 0x28),
        secondary: rgb(0xd6, 0x5d, 0x0e),
        surface: rgb(0x32, 0x30, 0x2f),
        elevated: rgb(0x3c, 0x38, 0x36),
        text: rgb(0xeb, 0xdb, 0xb2),
        text_dim: rgb(0xa8, 0x99, 0x84),
        border: rgb(0x50, 0x49, 0x45),
        danger: rgb(0xcc, 0x24, 0x1d),
        header_grad_from: rgb(0xb5, 0x76, 0x14),
        header_grad_to: rgb(0xfa, 0xbd, 0x2f),
    }
}

fn palette_solarized() -> Palette {
    Palette {
        primary: rgb(0x00, 0x2b, 0x36),
        secondary: rgb(0x26, 0x8b, 0xd2),
        surface: rgb(0x07, 0x36, 0x42),
        elevated: rgb(0x58, 0x6e, 0x75),
        text: rgb(0xee, 0xe8, 0xd5),
        text_dim: rgb(0x93, 0xa1, 0xa1),
        border: rgb(0x58, 0x6e, 0x75),
        danger: rgb(0xdc, 0x32, 0x2f),
        header_grad_from: rgb(0x07, 0x36, 0x42),
        header_grad_to: rgb(0x26, 0x8b, 0xd2),
    }
}
