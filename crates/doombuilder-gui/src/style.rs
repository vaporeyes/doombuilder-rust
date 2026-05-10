// ABOUTME: Win32-flavoured iced widget styles. Sharp corners, button-face
// ABOUTME: backgrounds, hover/pressed bevel cues. Used across the chrome
// ABOUTME: (toolbar, menu, panels, modal) so the editor reads like UDB.

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

const FACE: Color = Color {
    r: 0.941,
    g: 0.941,
    b: 0.941,
    a: 1.0,
}; // #F0F0F0
const SHADOW: Color = Color {
    r: 0.627,
    g: 0.627,
    b: 0.627,
    a: 1.0,
}; // #A0A0A0
const DARK_SHADOW: Color = Color {
    r: 0.412,
    g: 0.412,
    b: 0.412,
    a: 1.0,
}; // #696969
const LIGHT_FACE: Color = Color {
    r: 0.957,
    g: 0.957,
    b: 0.957,
    a: 1.0,
}; // #F5F5F5

// ---- Buttons ------------------------------------------------------------

/// Flat toolbar button (Win7-toolbar feel): no border or background until
/// hovered or pressed; sharp corners; black text.
pub fn win32_toolbar_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        text_color: Color::BLACK,
        border: Border {
            color: Color::TRANSPARENT,
            width: 1.0,
            radius: 0.0.into(),
        },
        background: None,
        shadow: Shadow::default(),
        snap: true,
    };

    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(Color::from_rgb8(225, 230, 232))),
            border: Border {
                color: Color::from_rgb8(153, 209, 255),
                ..base.border
            },
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(Color::from_rgb8(204, 228, 247))),
            border: Border {
                color: Color::from_rgb8(0, 84, 153),
                ..base.border
            },
            ..base
        },
        button::Status::Disabled => button::Style {
            text_color: Color::from_rgb8(160, 160, 160),
            ..base
        },
    }
}

/// Classic raised Win32 button: button-face fill, gray border, depth inverts on
/// press.
pub fn win32_standard_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        text_color: Color::BLACK,
        background: Some(Background::Color(FACE)),
        border: Border {
            color: SHADOW,
            width: 1.0,
            radius: 0.0.into(),
        },
        shadow: Shadow::default(),
        snap: true,
    };

    match status {
        button::Status::Active => base,
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(Color::from_rgb8(225, 225, 225))),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(Color::from_rgb8(210, 210, 210))),
            border: Border {
                color: DARK_SHADOW,
                ..base.border
            },
            ..base
        },
        button::Status::Disabled => button::Style {
            text_color: SHADOW,
            background: Some(Background::Color(Color::from_rgb8(245, 245, 245))),
            border: Border {
                color: Color::from_rgb8(220, 220, 220),
                ..base.border
            },
            ..base
        },
    }
}

/// Toggle button. When `on`, renders as a pressed standard button so the
/// "depressed" state is visually obvious for flag toggles.
pub fn win32_toggle_button(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let effective = if on {
            // While the user actively presses, keep the "pressed" look.
            match status {
                button::Status::Disabled => button::Status::Disabled,
                _ => button::Status::Pressed,
            }
        } else {
            status
        };
        win32_standard_button(theme, effective)
    }
}

// ---- Containers ---------------------------------------------------------

/// Plain Win32 panel: button-face fill, hairline gray border on the edges.
pub fn win32_panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(FACE)),
        border: Border {
            color: SHADOW,
            width: 1.0,
            radius: 0.0.into(),
        },
        text_color: Some(Color::BLACK),
        ..Default::default()
    }
}

/// Lighter "menu strip" Win32 panel.
pub fn win32_menu(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(LIGHT_FACE)),
        border: Border {
            color: SHADOW,
            width: 0.0,
            radius: 0.0.into(),
        },
        text_color: Some(Color::BLACK),
        ..Default::default()
    }
}

/// Status bar: face fill, top-edge highlight via a thin border.
pub fn win32_status_bar(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(FACE)),
        border: Border {
            color: SHADOW,
            width: 0.0,
            radius: 0.0.into(),
        },
        text_color: Some(Color::BLACK),
        ..Default::default()
    }
}

/// Sunken "well" container used by texture slots / sprite previews.
pub fn win32_well(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb8(255, 255, 255))),
        border: Border {
            color: DARK_SHADOW,
            width: 1.0,
            radius: 0.0.into(),
        },
        text_color: Some(Color::BLACK),
        ..Default::default()
    }
}

/// Inset side-panel container (raised look).
pub fn win32_side_panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(LIGHT_FACE)),
        border: Border {
            color: SHADOW,
            width: 1.0,
            radius: 0.0.into(),
        },
        text_color: Some(Color::BLACK),
        ..Default::default()
    }
}

/// Modal floating panel.
pub fn win32_modal_panel(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(FACE)),
        border: Border {
            color: DARK_SHADOW,
            width: 1.0,
            radius: 0.0.into(),
        },
        text_color: Some(Color::BLACK),
        ..Default::default()
    }
}

/// Modal backdrop (semi-transparent dim layer).
pub fn win32_modal_backdrop(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.45))),
        ..Default::default()
    }
}

/// Single-pixel separator line between toolbar groups.
pub fn win32_separator(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SHADOW)),
        ..Default::default()
    }
}

// ---- Text input ---------------------------------------------------------

pub fn win32_text_input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let base = text_input::Style {
        background: Background::Color(Color::WHITE),
        border: Border {
            color: DARK_SHADOW,
            width: 1.0,
            radius: 0.0.into(),
        },
        icon: Color::BLACK,
        placeholder: Color::from_rgb8(140, 140, 140),
        value: Color::BLACK,
        selection: Color::from_rgb8(49, 106, 197),
    };

    match status {
        text_input::Status::Active => base,
        text_input::Status::Hovered => text_input::Style {
            border: Border {
                color: Color::from_rgb8(0, 84, 153),
                ..base.border
            },
            ..base
        },
        text_input::Status::Focused { .. } => text_input::Style {
            border: Border {
                color: Color::from_rgb8(0, 120, 215),
                ..base.border
            },
            ..base
        },
        text_input::Status::Disabled => text_input::Style {
            background: Background::Color(Color::from_rgb8(240, 240, 240)),
            value: SHADOW,
            ..base
        },
    }
}
