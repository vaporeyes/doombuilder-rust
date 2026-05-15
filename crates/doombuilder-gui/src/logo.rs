// ABOUTME: Decodes the embedded app logo PNGs into RGBA for the OS window
// ABOUTME: icon and the in-app splash shown when no map is loaded.

// iced is built with `image-without-codecs`, so the `image` crate's decoders
// (and iced's `from_file_data` helper) are unavailable. We decode the PNGs
// ourselves with the `png` crate and feed raw RGBA to iced.

use std::sync::OnceLock;

use iced::widget::image;

/// Badge-only mark on a white plate, used as the OS window/dock icon.
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
/// Full "DoomBuilder" lockup on a white plate, shown in the empty viewport.
const SPLASH_PNG: &[u8] = include_bytes!("../assets/splash.png");

/// Decode an 8-bit PNG to `(rgba, width, height)`.
///
/// Handles the only color types our committed assets produce: RGBA8
/// (pass-through) and RGB8 (opaque alpha inserted). Returns `None` on any
/// unexpected encoding so callers can degrade gracefully.
fn decode_rgba(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());

    if info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(buf.len() / 3 * 4);
            for px in buf.chunks_exact(3) {
                out.extend_from_slice(px);
                out.push(0xFF);
            }
            out
        }
        _ => return None,
    };
    Some((rgba, info.width, info.height))
}

/// Window icon for `iced::window::Settings`. `None` if decoding fails — the
/// app still runs, just without a custom icon.
pub fn window_icon() -> Option<iced::window::Icon> {
    let (rgba, w, h) = decode_rgba(ICON_PNG)?;
    iced::window::icon::from_rgba(rgba, w, h).ok()
}

/// Splash image handle, decoded once and cached for reuse across renders.
pub fn splash_handle() -> Option<image::Handle> {
    static SPLASH: OnceLock<Option<image::Handle>> = OnceLock::new();
    SPLASH
        .get_or_init(|| {
            let (rgba, w, h) = decode_rgba(SPLASH_PNG)?;
            Some(image::Handle::from_rgba(w, h, rgba))
        })
        .clone()
}

/// Badge-only mark handle (the window-icon art) for in-app use such as the
/// About modal. Decoded once and cached.
pub fn badge_handle() -> Option<image::Handle> {
    static BADGE: OnceLock<Option<image::Handle>> = OnceLock::new();
    BADGE
        .get_or_init(|| {
            let (rgba, w, h) = decode_rgba(ICON_PNG)?;
            Some(image::Handle::from_rgba(w, h, rgba))
        })
        .clone()
}
