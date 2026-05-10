// ABOUTME: Inline SVG bitmap-style icons for toolbar buttons. Each icon is a
// ABOUTME: tiny 16x16 vector glyph rendered via iced's svg widget. Self-
// ABOUTME: contained (no font / no network); recolors via currentColor.

use iced::widget::svg::{self, Handle};
use iced::widget::{button, container, svg as svg_widget, text, tooltip, Tooltip};
use iced::{Color, Element, Length, Theme};

use crate::style;

/// One SVG per icon — 16×16, simple paths, `currentColor` fill so the theme
/// can drive color (we leave it black; matches Win32 chrome).
pub const FOLDER_OPEN: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M1 4 h4 l1 1 h9 v8 h-14 z' fill='none' stroke='black' stroke-width='1.2' stroke-linejoin='round'/></svg>"#;

pub const NEW_DOC: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M3 1 h7 l3 3 v11 h-10 z' fill='none' stroke='black' stroke-width='1.2' stroke-linejoin='round'/><polyline points='10,1 10,4 13,4' fill='none' stroke='black' stroke-width='1.0' stroke-linejoin='round'/><line x1='8' y1='8' x2='8' y2='13' stroke='black' stroke-width='1.4' stroke-linecap='round'/><line x1='5.5' y1='10.5' x2='10.5' y2='10.5' stroke='black' stroke-width='1.4' stroke-linecap='round'/></svg>"#;

pub const SAVE_DISK: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><rect x='2' y='2' width='12' height='12' fill='none' stroke='black' stroke-width='1.2'/><rect x='4' y='2' width='8' height='4' fill='black'/><rect x='5' y='9' width='6' height='4' fill='none' stroke='black' stroke-width='1.0'/></svg>"#;

pub const VIEW_2D: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><rect x='2' y='2' width='12' height='12' fill='none' stroke='black' stroke-width='1.2'/><line x1='6' y1='2' x2='6' y2='14' stroke='black' stroke-width='0.8'/><line x1='10' y1='2' x2='10' y2='14' stroke='black' stroke-width='0.8'/><line x1='2' y1='6' x2='14' y2='6' stroke='black' stroke-width='0.8'/><line x1='2' y1='10' x2='14' y2='10' stroke='black' stroke-width='0.8'/></svg>"#;

pub const VIEW_3D: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><polygon points='3,5 8,2 13,5 13,11 8,14 3,11' fill='none' stroke='black' stroke-width='1.2' stroke-linejoin='round'/><line x1='8' y1='2' x2='8' y2='8' stroke='black' stroke-width='1.0'/><line x1='3' y1='5' x2='8' y2='8' stroke='black' stroke-width='1.0'/><line x1='13' y1='5' x2='8' y2='8' stroke='black' stroke-width='1.0'/></svg>"#;

pub const VERTEX: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><line x1='2' y1='14' x2='14' y2='2' stroke='black' stroke-width='1.0'/><circle cx='8' cy='8' r='3' fill='black'/></svg>"#;

pub const LINEDEF: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><line x1='2' y1='13' x2='14' y2='3' stroke='black' stroke-width='2.0'/><circle cx='2' cy='13' r='1.5' fill='black'/><circle cx='14' cy='3' r='1.5' fill='black'/></svg>"#;

pub const SECTOR: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><polygon points='2,12 4,4 12,3 14,11 9,14' fill='black' fill-opacity='0.25' stroke='black' stroke-width='1.2' stroke-linejoin='round'/></svg>"#;

pub const THING: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><circle cx='8' cy='8' r='5' fill='none' stroke='black' stroke-width='1.2'/><line x1='8' y1='8' x2='14' y2='8' stroke='black' stroke-width='1.5'/></svg>"#;

pub const DRAW_PEN: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><polygon points='2,14 4,12 12,4 14,6 6,14' fill='none' stroke='black' stroke-width='1.2' stroke-linejoin='round'/><line x1='10' y1='6' x2='12' y2='8' stroke='black' stroke-width='1.0'/></svg>"#;

pub const MAKE_SECTOR: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><polygon points='3,12 5,4 11,3 13,10 9,14' fill='black' fill-opacity='0.6' stroke='black' stroke-width='1.0' stroke-linejoin='round'/><text x='8' y='10' text-anchor='middle' font-family='sans-serif' font-size='8' font-weight='bold' fill='white'>+</text></svg>"#;

pub const TEXTURES: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><rect x='2' y='2' width='12' height='12' fill='none' stroke='black' stroke-width='1.2'/><rect x='2' y='2' width='6' height='6' fill='black' fill-opacity='0.4'/><rect x='8' y='8' width='6' height='6' fill='black' fill-opacity='0.4'/></svg>"#;

pub const LOAD_RESOURCES: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><rect x='2' y='3' width='9' height='9' fill='none' stroke='black' stroke-width='1.0'/><rect x='4' y='5' width='9' height='9' fill='white' stroke='black' stroke-width='1.0'/><rect x='4' y='5' width='4' height='4' fill='black' fill-opacity='0.4'/><rect x='9' y='10' width='4' height='4' fill='black' fill-opacity='0.4'/></svg>"#;

pub const SETTINGS_GEAR: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><circle cx='8' cy='8' r='2.5' fill='none' stroke='black' stroke-width='1.2'/><circle cx='8' cy='8' r='5' fill='none' stroke='black' stroke-width='1.2' stroke-dasharray='2 1.5'/></svg>"#;

pub const PLAY_TRIANGLE: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><polygon points='4,3 13,8 4,13' fill='black' stroke='black' stroke-width='1.0' stroke-linejoin='round'/></svg>"#;

pub const UNDO: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M3 7 h7 a3 3 0 0 1 0 6 h-2' fill='none' stroke='black' stroke-width='1.4' stroke-linecap='round'/><polyline points='6,4 3,7 6,10' fill='none' stroke='black' stroke-width='1.4' stroke-linecap='round' stroke-linejoin='round'/></svg>"#;

pub const REDO: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><path d='M13 7 h-7 a3 3 0 0 0 0 6 h2' fill='none' stroke='black' stroke-width='1.4' stroke-linecap='round'/><polyline points='10,4 13,7 10,10' fill='none' stroke='black' stroke-width='1.4' stroke-linecap='round' stroke-linejoin='round'/></svg>"#;

pub const SPLIT_LINE: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><line x1='2' y1='13' x2='6' y2='8' stroke='black' stroke-width='1.6' stroke-linecap='round'/><line x1='10' y1='8' x2='14' y2='3' stroke='black' stroke-width='1.6' stroke-linecap='round'/><circle cx='8' cy='8' r='2' fill='black'/></svg>"#;

pub const MERGE_VERTS: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><circle cx='3' cy='3' r='1.6' fill='black'/><circle cx='13' cy='13' r='1.6' fill='black'/><line x1='3' y1='3' x2='8' y2='8' stroke='black' stroke-width='1.2' stroke-dasharray='1.5 1.5'/><line x1='13' y1='13' x2='8' y2='8' stroke='black' stroke-width='1.2' stroke-dasharray='1.5 1.5'/><circle cx='8' cy='8' r='2.4' fill='none' stroke='black' stroke-width='1.4'/></svg>"#;

pub const FLIP_LINE: &str = r#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'><line x1='2' y1='13' x2='14' y2='3' stroke='black' stroke-width='1.6'/><circle cx='2' cy='13' r='1.4' fill='black'/><polygon points='14,3 11,2 12,5' fill='black'/><polyline points='5,5 8,8 5,8' fill='none' stroke='black' stroke-width='1.0' stroke-linejoin='round'/><polyline points='11,11 8,8 11,8' fill='none' stroke='black' stroke-width='1.0' stroke-linejoin='round'/></svg>"#;

fn handle(s: &'static str) -> Handle {
    Handle::from_memory(s.as_bytes().to_vec())
}

fn icon_glyph(svg_str: &'static str) -> svg_widget::Svg<'static, Theme> {
    svg_widget(handle(svg_str))
        .width(Length::Fixed(16.0))
        .height(Length::Fixed(16.0))
        .style(svg_currentcolor_style)
}

/// Toggle-style icon button (raised / depressed depending on `pressed`).
pub fn icon_btn<'a, Msg: 'a + Clone + 'static>(
    svg_str: &'static str,
    tip: &'a str,
    msg: Msg,
    pressed: bool,
) -> Element<'a, Msg> {
    let btn = button(icon_glyph(svg_str))
        .padding(2)
        .style(style::win32_toggle_button(pressed))
        .on_press(msg);
    wrap_tooltip(btn.into(), tip)
}

/// Command-style icon button (always raised) for one-shot actions.
pub fn icon_cmd_btn<'a, Msg: 'a + Clone + 'static>(
    svg_str: &'static str,
    tip: &'a str,
    msg: Msg,
) -> Element<'a, Msg> {
    let btn = button(icon_glyph(svg_str))
        .padding(2)
        .style(style::win32_standard_button)
        .on_press(msg);
    wrap_tooltip(btn.into(), tip)
}

fn wrap_tooltip<'a, Msg: 'a>(content: Element<'a, Msg>, tip: &'a str) -> Element<'a, Msg> {
    let tip_box = container(text(tip).size(12))
        .padding(4)
        .style(tooltip_box_style);
    Tooltip::new(content, tip_box, tooltip::Position::Bottom)
        .gap(2)
        .into()
}

fn svg_currentcolor_style(_theme: &Theme, _status: svg::Status) -> svg::Style {
    svg::Style {
        color: Some(Color::BLACK),
    }
}

fn tooltip_box_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(Color::from_rgb8(255, 255, 220))),
        text_color: Some(Color::BLACK),
        border: iced::Border {
            color: Color::from_rgb8(120, 120, 120),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

