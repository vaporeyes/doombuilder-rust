// ABOUTME: Palette-driven widget styles inspired by sniffnet. Rounded surfaces,
// ABOUTME: pill buttons, gradient header. Active palette is read from the
// ABOUTME: `palette::active()` global so widget closures stay zero-arg.

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

use crate::palette::{self, Palette};

const RADIUS_PANEL: f32 = 12.0;
const RADIUS_MODAL: f32 = 16.0;
const RADIUS_INPUT: f32 = 8.0;
const RADIUS_TOOLBAR_BTN: f32 = 8.0;
const RADIUS_PILL: f32 = 100.0;
const BORDER_HAIRLINE: f32 = 1.0;

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

// ---- Buttons ------------------------------------------------------------

/// Flat icon toolbar button. Transparent until hovered; subtle rounded
/// elevated background on hover; secondary tint on press.
pub fn win32_toolbar_button(_theme: &Theme, status: button::Status) -> button::Style {
    let p = palette::active();
    let bg = match status {
        button::Status::Hovered => Background::Color(p.elevated),
        button::Status::Pressed => Background::Color(with_alpha(p.secondary, 0.35)),
        _ => Background::Color(Color::TRANSPARENT),
    };
    button::Style {
        text_color: p.text,
        background: Some(bg),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS_TOOLBAR_BTN.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Pill-shaped primary button. Secondary fill, contrasting text. Subtle
/// elevation on hover.
pub fn win32_standard_button(_theme: &Theme, status: button::Status) -> button::Style {
    let p = palette::active();
    let bg = match status {
        button::Status::Hovered => Background::Color(blend(p.secondary, p.text, 0.12)),
        button::Status::Pressed => Background::Color(blend(p.secondary, p.primary, 0.2)),
        button::Status::Disabled => Background::Color(p.elevated),
        _ => Background::Color(p.secondary),
    };
    let text_color = match status {
        button::Status::Disabled => p.text_dim,
        _ => contrast_text(p.secondary, &p),
    };
    button::Style {
        text_color,
        background: Some(bg),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: RADIUS_PILL.into(),
        },
        shadow: match status {
            button::Status::Hovered => Shadow {
                color: with_alpha(Color::BLACK, 0.25),
                offset: Vector::new(0.0, 2.0),
                blur_radius: 6.0,
            },
            _ => Shadow::default(),
        },
        snap: true,
    }
}

/// Toggle button: secondary fill when `on`, neutral elevated when off.
/// Used for edit-mode switchers (Vertex/Linedef/Sector/Thing) and draw.
pub fn win32_toggle_button(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let p = palette::active();
        let (bg, text_color) = if on {
            (Background::Color(p.secondary), contrast_text(p.secondary, &p))
        } else {
            let face = match status {
                button::Status::Hovered => p.elevated,
                button::Status::Pressed => blend(p.elevated, p.secondary, 0.4),
                _ => with_alpha(p.elevated, 0.6),
            };
            (Background::Color(face), p.text)
        };
        button::Style {
            text_color,
            background: Some(bg),
            border: Border {
                color: if on { Color::TRANSPARENT } else { p.border },
                width: if on { 0.0 } else { BORDER_HAIRLINE },
                radius: RADIUS_TOOLBAR_BTN.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

// ---- Containers ---------------------------------------------------------

/// Generic rounded panel surface used for bottom-bar inspector wells.
pub fn win32_panel(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.surface)),
        border: Border {
            color: p.border,
            width: BORDER_HAIRLINE,
            radius: RADIUS_PANEL.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Top menu strip — gradient from `header_grad_from` to `header_grad_to`.
/// Falls back to a flat surface when gradients are problematic; iced 0.14
/// accepts gradients but for portability we render a vertical fade by
/// blending two solid passes via a slightly off-axis box-shadow trick.
/// Here we just use the gradient-to color as a flat band, with a hairline.
pub fn win32_menu(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(contrast_text(p.header_grad_to, &p)),
        background: Some(Background::Gradient(iced::Gradient::Linear(
            iced::gradient::Linear::new(0.0)
                .add_stop(0.0, p.header_grad_from)
                .add_stop(1.0, p.header_grad_to),
        ))),
        border: Border {
            color: with_alpha(Color::BLACK, 0.2),
            width: 0.0,
            radius: 0.0.into(),
        },
        shadow: Shadow {
            color: with_alpha(Color::BLACK, 0.25),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 4.0,
        },
        snap: true,
    }
}

/// Status bar — distinct elevated band so it reads as separate from canvas.
pub fn win32_status_bar(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(p.text_dim),
        background: Some(Background::Color(p.surface)),
        border: Border {
            color: p.border,
            width: BORDER_HAIRLINE,
            radius: 0.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Inset "well" used inside inspectors for input groups.
pub fn win32_well(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.elevated)),
        border: Border {
            color: p.border,
            width: BORDER_HAIRLINE,
            radius: RADIUS_INPUT.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Bottom dock that hosts inspectors — slightly recessed primary surface.
pub fn win32_side_panel(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.primary)),
        border: Border {
            color: p.border,
            width: BORDER_HAIRLINE,
            radius: 0.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Modal foreground card — pronounced rounded corners + shadow.
pub fn win32_modal_panel(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(p.surface)),
        border: Border {
            color: p.border,
            width: BORDER_HAIRLINE,
            radius: RADIUS_MODAL.into(),
        },
        shadow: Shadow {
            color: with_alpha(Color::BLACK, 0.5),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 24.0,
        },
        snap: true,
    }
}

/// Backdrop for 3D viewports. Picks a desaturated, slightly-bluer surface
/// than `surface` so brown walls and dark floors stay readable against it
/// and the viewport reads as a distinct area on the chrome.
pub fn viewport_3d_bg(_theme: &Theme) -> container::Style {
    let p = palette::active();
    // Blend the palette's surface 60% toward steel blue.
    let steel = Color {
        r: 0.32,
        g: 0.38,
        b: 0.45,
        a: 1.0,
    };
    let bg = blend(p.surface, steel, 0.65);
    container::Style {
        text_color: Some(p.text),
        background: Some(Background::Color(bg)),
        border: Border {
            color: p.border,
            width: BORDER_HAIRLINE,
            radius: 0.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Scrim behind a modal: 60% black so the canvas dims clearly.
pub fn win32_modal_backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: None,
        background: Some(Background::Color(with_alpha(Color::BLACK, 0.6))),
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// 1-px separator coloured per palette. Dimmed so it reads as a hairline
/// rather than a chunky bar against the gradient header.
pub fn win32_separator(_theme: &Theme) -> container::Style {
    let p = palette::active();
    container::Style {
        text_color: None,
        background: Some(Background::Color(with_alpha(p.border, 0.55))),
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}

// ---- Text input ---------------------------------------------------------

pub fn win32_text_input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let p = palette::active();
    let bg = p.elevated;
    let border_color = match status {
        text_input::Status::Focused { .. } => p.secondary,
        _ => p.border,
    };
    text_input::Style {
        background: Background::Color(bg),
        border: Border {
            color: border_color,
            width: BORDER_HAIRLINE,
            radius: RADIUS_INPUT.into(),
        },
        icon: p.text_dim,
        placeholder: p.text_dim,
        value: p.text,
        selection: with_alpha(p.secondary, 0.45),
    }
}

// ---- Helpers ------------------------------------------------------------

fn blend(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let i = 1.0 - t;
    Color {
        r: a.r * i + b.r * t,
        g: a.g * i + b.g * t,
        b: a.b * i + b.b * t,
        a: a.a * i + b.a * t,
    }
}

/// Pick black or the palette's body-text color for highest contrast against
/// `bg`. Used so primary buttons stay readable across all themes.
fn contrast_text(bg: Color, p: &Palette) -> Color {
    let luma = 0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b;
    if luma > 0.55 {
        // Light background → dark text.
        if relative_luma(p.primary) < relative_luma(p.text) {
            p.primary
        } else {
            p.text
        }
    } else {
        // Dark background → light text.
        if relative_luma(p.text) > relative_luma(p.primary) {
            p.text
        } else {
            p.primary
        }
    }
}

fn relative_luma(c: Color) -> f32 {
    0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
}
