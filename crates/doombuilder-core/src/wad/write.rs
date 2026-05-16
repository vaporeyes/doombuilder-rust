// ABOUTME: Pack a list of named lumps into PWAD bytes. Header + bodies +
// ABOUTME: directory, all little-endian. Names are uppercased and null-padded
// ABOUTME: to 8 bytes; over-long names are truncated.

const HEADER_SIZE: usize = 12;
const DIR_ENTRY_SIZE: usize = 16;

pub struct LumpEntry<'a> {
    pub name: &'a str,
    pub data: Vec<u8>,
}

pub fn write_pwad(lumps: &[LumpEntry]) -> Vec<u8> {
    let body_size: usize = lumps.iter().map(|l| l.data.len()).sum();
    let dir_offset = HEADER_SIZE + body_size;
    let total = dir_offset + lumps.len() * DIR_ENTRY_SIZE;
    let mut out = Vec::with_capacity(total);

    out.extend_from_slice(b"PWAD");
    out.extend_from_slice(&(lumps.len() as u32).to_le_bytes());
    out.extend_from_slice(&(dir_offset as u32).to_le_bytes());

    let mut entries: Vec<(u32, u32, [u8; 8])> = Vec::with_capacity(lumps.len());
    let mut cursor = HEADER_SIZE as u32;
    for lump in lumps {
        let mut padded = [0u8; 8];
        let upper: Vec<u8> = lump
            .name
            .bytes()
            .take(8)
            .map(|b| b.to_ascii_uppercase())
            .collect();
        padded[..upper.len()].copy_from_slice(&upper);
        let size = lump.data.len() as u32;
        entries.push((cursor, size, padded));
        out.extend_from_slice(&lump.data);
        cursor += size;
    }
    for (filepos, size, name) in entries {
        out.extend_from_slice(&filepos.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name);
    }
    out
}
