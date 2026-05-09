// ABOUTME: WAD file header (12 bytes): magic, lump count, directory offset.
// ABOUTME: Distinguishes IWAD (full game) from PWAD (patch / mod) on the magic.

use crate::error::{Error, Result};

pub const HEADER_SIZE: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WadKind {
    Iwad,
    Pwad,
}

#[derive(Debug, Clone, Copy)]
pub struct WadHeader {
    pub kind: WadKind,
    pub num_lumps: u32,
    pub directory_offset: u32,
}

impl WadHeader {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_SIZE {
            return Err(Error::Truncated {
                offset: 0,
                expected: HEADER_SIZE as u64,
                actual: bytes.len() as u64,
            });
        }
        let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
        let kind = match &magic {
            b"IWAD" => WadKind::Iwad,
            b"PWAD" => WadKind::Pwad,
            _ => return Err(Error::BadWadMagic(magic)),
        };
        let num_lumps = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        let directory_offset = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        Ok(Self {
            kind,
            num_lumps,
            directory_offset,
        })
    }
}
