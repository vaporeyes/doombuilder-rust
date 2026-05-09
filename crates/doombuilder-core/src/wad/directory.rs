// ABOUTME: WAD directory entry (16 bytes): file offset, size, 8-byte ASCII name.
// ABOUTME: Names are null-padded; lookups use the trimmed UTF-8 view.

use bytemuck::{Pod, Zeroable};

use crate::error::{Error, Result};

use super::header::WadHeader;

pub const ENTRY_SIZE: usize = 16;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct DirEntry {
    pub filepos: u32,
    pub size: u32,
    pub name: [u8; 8],
}

impl DirEntry {
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(8);
        std::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
}

pub fn parse(bytes: &[u8], header: &WadHeader) -> Result<Vec<DirEntry>> {
    let offset = header.directory_offset as usize;
    let count = header.num_lumps as usize;
    let needed = count
        .checked_mul(ENTRY_SIZE)
        .ok_or_else(|| Error::Truncated {
            offset: offset as u64,
            expected: u64::MAX,
            actual: bytes.len() as u64,
        })?;
    let end = offset
        .checked_add(needed)
        .ok_or_else(|| Error::Truncated {
            offset: offset as u64,
            expected: needed as u64,
            actual: bytes.len() as u64,
        })?;
    if end > bytes.len() {
        return Err(Error::Truncated {
            offset: offset as u64,
            expected: needed as u64,
            actual: bytes.len() as u64,
        });
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let base = offset + i * ENTRY_SIZE;
        let chunk = &bytes[base..base + ENTRY_SIZE];
        let entry = DirEntry {
            filepos: u32::from_le_bytes(chunk[0..4].try_into().unwrap()),
            size: u32::from_le_bytes(chunk[4..8].try_into().unwrap()),
            name: chunk[8..16].try_into().unwrap(),
        };
        out.push(entry);
    }
    Ok(out)
}
