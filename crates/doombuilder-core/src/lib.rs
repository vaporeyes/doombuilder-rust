// ABOUTME: Public API surface for the doombuilder core library.
// ABOUTME: Re-exports map format, WAD reader, and archive opener.

pub mod archive;
pub mod config;
pub mod edit;
pub mod error;
pub mod format;
pub mod map;
pub mod wad;

pub use archive::{open, Asset};
pub use error::Error;
pub use format::MapFormat;
pub use map::{detect_format, load_auto, load_doom, load_hexen, Map};
pub use wad::Wad;
