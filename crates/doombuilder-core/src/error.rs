// ABOUTME: Single error type for asset I/O and parsing failures.
// ABOUTME: Wraps std::io and zip errors and adds WAD-specific variants.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("not a WAD: bad magic {0:?}")]
    BadWadMagic([u8; 4]),

    #[error("WAD truncated: expected {expected} bytes at offset {offset}, file has {actual}")]
    Truncated {
        offset: u64,
        expected: u64,
        actual: u64,
    },

    #[error("lump '{name}' size {size} is not a multiple of {stride}")]
    BadLumpSize {
        name: String,
        size: usize,
        stride: usize,
    },

    #[error("lump '{name}' starts at unaligned offset {offset} (need {align}-byte alignment for zero-copy)")]
    Unaligned {
        name: String,
        offset: usize,
        align: usize,
    },

    #[error("archive contains no WAD or PK3")]
    NoPlayableAsset,

    #[error("nested archives deeper than one level are not supported")]
    NestingTooDeep,

    #[error("map '{0}' not found in WAD")]
    MapNotFound(String),

    #[error("map '{map}' is missing required lump '{lump}'")]
    MissingMapLump { map: String, lump: String },

    #[error("bad {what}: index {index} out of range (len {len})")]
    BadReference {
        what: String,
        index: usize,
        len: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
