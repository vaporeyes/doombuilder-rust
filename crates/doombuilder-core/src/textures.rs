// ABOUTME: Doom texture pipeline: PLAYPAL palette, PNAMES patch list,
// ABOUTME: TEXTURE1/TEXTURE2 composite definitions, Doom-format patches, and
// ABOUTME: flats. Composes everything into RGBA images consumable by any GPU.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::wad::{DirEntry, Wad};

const PALETTE_BYTES: usize = 768;
const FLAT_BYTES: usize = 4096;
const FLAT_WIDTH: u16 = 64;
const FLAT_HEIGHT: u16 = 64;

#[derive(Debug, Clone)]
pub struct Palette(pub [[u8; 3]; 256]);

impl Palette {
    pub fn rgba(&self, index: u8) -> [u8; 4] {
        let [r, g, b] = self.0[index as usize];
        [r, g, b, 255]
    }
}

#[derive(Debug, Clone)]
pub struct TextureImage {
    pub width: u16,
    pub height: u16,
    /// RGBA bytes, row-major, top-to-bottom.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TextureSet {
    pub palette: Palette,
    pub textures: HashMap<String, TextureImage>,
    pub flats: HashMap<String, TextureImage>,
    pub sprites: HashMap<String, TextureImage>,
    /// Textures that referenced a missing patch; surfaced for diagnostics.
    pub diagnostics: Vec<String>,
}

impl TextureSet {
    /// Build the texture set from a WAD. Returns an empty set with diagnostics
    /// rather than failing if PLAYPAL or PNAMES are missing.
    pub fn load_from_wad(wad: &Wad) -> Self {
        let mut diagnostics = Vec::new();

        let palette = match Self::load_palette(wad) {
            Ok(p) => p,
            Err(e) => {
                diagnostics.push(format!("PLAYPAL: {e}"));
                return Self::empty(diagnostics);
            }
        };

        let pnames = match Self::load_pnames(wad) {
            Ok(p) => p,
            Err(e) => {
                diagnostics.push(format!("PNAMES: {e}"));
                Vec::new()
            }
        };

        let mut patches: HashMap<usize, Patch> = HashMap::new();
        for (idx, name) in pnames.iter().enumerate() {
            if let Some(entry) = wad.find(&name.to_ascii_uppercase()) {
                match parse_patch(wad.lump_bytes(entry).unwrap_or(&[])) {
                    Ok(p) => {
                        patches.insert(idx, p);
                    }
                    Err(_) => {}
                }
            }
        }

        let mut textures: HashMap<String, TextureImage> = HashMap::new();
        for lump_name in ["TEXTURE1", "TEXTURE2"] {
            if let Some(entry) = wad.find(lump_name) {
                let bytes = match wad.lump_bytes(entry) {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                match parse_texture_lump(bytes) {
                    Ok(defs) => {
                        for def in defs {
                            let name = def.name.clone();
                            let mut missing_patch = false;
                            let img = compose_texture(&def, &patches, &palette, &mut missing_patch);
                            if missing_patch {
                                diagnostics.push(format!("texture {name}: missing patch"));
                            }
                            textures.insert(name, img);
                        }
                    }
                    Err(e) => diagnostics.push(format!("{lump_name}: {e}")),
                }
            }
        }

        // Flats: between F_START / F_END, FF_START / FF_END, or simply any 4096-byte
        // lump under those markers. Vanilla layout uses F_START..F_END exclusive.
        let mut flats: HashMap<String, TextureImage> = HashMap::new();
        let dir = wad.directory();
        let mut in_flats = false;
        for entry in dir {
            let n = entry.name_str();
            let upper = n.to_ascii_uppercase();
            match upper.as_str() {
                "F_START" | "FF_START" | "F1_START" | "F2_START" | "F3_START" => {
                    in_flats = true;
                    continue;
                }
                "F_END" | "FF_END" | "F1_END" | "F2_END" | "F3_END" => {
                    in_flats = false;
                    continue;
                }
                _ => {}
            }
            if !in_flats {
                continue;
            }
            if entry.size as usize != FLAT_BYTES {
                continue;
            }
            if let Ok(bytes) = wad.lump_bytes(entry) {
                let img = compose_flat(bytes, &palette);
                flats.insert(upper, img);
            }
        }

        // Sprites: between S_START/S_END (or SS_START/SS_END). Each lump is a
        // single Doom patch; compose with the palette into an RGBA image.
        let mut sprites: HashMap<String, TextureImage> = HashMap::new();
        let mut in_sprites = false;
        for entry in dir {
            let n = entry.name_str();
            let upper = n.to_ascii_uppercase();
            match upper.as_str() {
                "S_START" | "SS_START" => {
                    in_sprites = true;
                    continue;
                }
                "S_END" | "SS_END" => {
                    in_sprites = false;
                    continue;
                }
                _ => {}
            }
            if !in_sprites || entry.size == 0 {
                continue;
            }
            if let Ok(bytes) = wad.lump_bytes(entry) {
                if let Some(img) = compose_sprite(bytes, &palette) {
                    insert_sprite_aliases(&mut sprites, &upper, img);
                }
            }
        }

        Self {
            palette,
            textures,
            flats,
            sprites,
            diagnostics,
        }
    }

    pub fn empty(diagnostics: Vec<String>) -> Self {
        Self {
            palette: Palette([[0; 3]; 256]),
            textures: HashMap::new(),
            flats: HashMap::new(),
            sprites: HashMap::new(),
            diagnostics,
        }
    }

    pub fn sprite(&self, name: &str) -> Option<&TextureImage> {
        self.sprites.get(&name.to_ascii_uppercase())
    }

    pub fn texture_or_flat(&self, name: &str) -> Option<&TextureImage> {
        let upper = name.to_ascii_uppercase();
        self.textures.get(&upper).or_else(|| self.flats.get(&upper))
    }

    fn load_palette(wad: &Wad) -> Result<Palette> {
        let entry = wad
            .find("PLAYPAL")
            .ok_or_else(|| Error::MapNotFound("PLAYPAL".into()))?;
        let bytes = wad.lump_bytes(entry)?;
        if bytes.len() < PALETTE_BYTES {
            return Err(Error::Truncated {
                offset: 0,
                expected: PALETTE_BYTES as u64,
                actual: bytes.len() as u64,
            });
        }
        let mut entries = [[0u8; 3]; 256];
        for i in 0..256 {
            let off = i * 3;
            entries[i] = [bytes[off], bytes[off + 1], bytes[off + 2]];
        }
        Ok(Palette(entries))
    }

    fn load_pnames(wad: &Wad) -> Result<Vec<String>> {
        let entry = wad
            .find("PNAMES")
            .ok_or_else(|| Error::MapNotFound("PNAMES".into()))?;
        let bytes = wad.lump_bytes(entry)?;
        if bytes.len() < 4 {
            return Err(Error::Truncated {
                offset: 0,
                expected: 4,
                actual: bytes.len() as u64,
            });
        }
        let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let needed = 4 + count * 8;
        if bytes.len() < needed {
            return Err(Error::Truncated {
                offset: 4,
                expected: needed as u64,
                actual: bytes.len() as u64,
            });
        }
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let off = 4 + i * 8;
            let raw = &bytes[off..off + 8];
            let end = raw.iter().position(|&b| b == 0).unwrap_or(8);
            out.push(
                std::str::from_utf8(&raw[..end])
                    .unwrap_or("")
                    .to_ascii_uppercase(),
            );
        }
        Ok(out)
    }
}

#[derive(Debug, Clone)]
struct TextureDef {
    name: String,
    width: u16,
    height: u16,
    patches: Vec<PatchUse>,
}

#[derive(Debug, Clone)]
struct PatchUse {
    x_origin: i16,
    y_origin: i16,
    patch_index: u16,
}

fn parse_texture_lump(bytes: &[u8]) -> Result<Vec<TextureDef>> {
    if bytes.len() < 4 {
        return Err(Error::Truncated {
            offset: 0,
            expected: 4,
            actual: bytes.len() as u64,
        });
    }
    let count = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let header_end = 4 + count * 4;
    if bytes.len() < header_end {
        return Err(Error::Truncated {
            offset: 4,
            expected: header_end as u64,
            actual: bytes.len() as u64,
        });
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let offset = u32::from_le_bytes(bytes[4 + i * 4..8 + i * 4].try_into().unwrap()) as usize;
        if offset + 22 > bytes.len() {
            continue;
        }
        let name_bytes = &bytes[offset..offset + 8];
        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(8);
        let name = std::str::from_utf8(&name_bytes[..end])
            .unwrap_or("")
            .to_ascii_uppercase();
        let width = i16::from_le_bytes(bytes[offset + 12..offset + 14].try_into().unwrap()) as u16;
        let height = i16::from_le_bytes(bytes[offset + 14..offset + 16].try_into().unwrap()) as u16;
        let patchcount = i16::from_le_bytes(bytes[offset + 20..offset + 22].try_into().unwrap())
            .max(0) as usize;
        let mut patches = Vec::with_capacity(patchcount);
        let p_start = offset + 22;
        let p_end = p_start + patchcount * 10;
        if p_end > bytes.len() {
            continue;
        }
        for p in 0..patchcount {
            let po = p_start + p * 10;
            patches.push(PatchUse {
                x_origin: i16::from_le_bytes(bytes[po..po + 2].try_into().unwrap()),
                y_origin: i16::from_le_bytes(bytes[po + 2..po + 4].try_into().unwrap()),
                patch_index: u16::from_le_bytes(bytes[po + 4..po + 6].try_into().unwrap()),
            });
        }
        out.push(TextureDef {
            name,
            width,
            height,
            patches,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct PatchPost {
    top: u16,
    pixels: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Patch {
    width: u16,
    height: u16,
    columns: Vec<Vec<PatchPost>>,
}

fn parse_patch(bytes: &[u8]) -> Result<Patch> {
    if bytes.len() < 8 {
        return Err(Error::Truncated {
            offset: 0,
            expected: 8,
            actual: bytes.len() as u64,
        });
    }
    let width = u16::from_le_bytes(bytes[0..2].try_into().unwrap());
    let height = u16::from_le_bytes(bytes[2..4].try_into().unwrap());
    let cols_table_end = 8 + (width as usize) * 4;
    if bytes.len() < cols_table_end {
        return Err(Error::Truncated {
            offset: 8,
            expected: cols_table_end as u64,
            actual: bytes.len() as u64,
        });
    }
    let mut columns: Vec<Vec<PatchPost>> = Vec::with_capacity(width as usize);
    for col in 0..width as usize {
        let off_bytes = &bytes[8 + col * 4..12 + col * 4];
        let mut p = u32::from_le_bytes(off_bytes.try_into().unwrap()) as usize;
        let mut posts = Vec::new();
        while p < bytes.len() {
            let topdelta = bytes[p];
            if topdelta == 0xFF {
                break;
            }
            if p + 2 >= bytes.len() {
                break;
            }
            let length = bytes[p + 1] as usize;
            // Skip first padding byte at p + 2.
            let pixels_start = p + 3;
            let pixels_end = pixels_start + length;
            if pixels_end >= bytes.len() {
                break;
            }
            let pixels = bytes[pixels_start..pixels_end].to_vec();
            posts.push(PatchPost {
                top: topdelta as u16,
                pixels,
            });
            // Trailing padding byte + advance to next post header.
            p = pixels_end + 1;
        }
        columns.push(posts);
    }
    Ok(Patch {
        width,
        height,
        columns,
    })
}

/// Doom sprite lumps come in two forms:
/// - 6-char names like `PLAYA1` → one frame/rotation, store as-is.
/// - 8-char names like `PLAYF1F8` → the same image data covers two rotations
///   (the second half indicates a mirror). Store the bitmap under BOTH 6-char
///   keys so a config entry asking for either rotation can find it.
fn insert_sprite_aliases(
    sprites: &mut HashMap<String, TextureImage>,
    name: &str,
    img: TextureImage,
) {
    if name.len() >= 8 {
        let base = &name[..4];
        let first = &name[4..6];
        let second = &name[6..8];
        let key_a = format!("{}{}", base, first);
        let key_b = format!("{}{}", base, second);
        sprites.insert(key_a, img.clone());
        sprites.insert(key_b, img);
    } else {
        sprites.insert(name.to_string(), img);
    }
}

fn compose_sprite(bytes: &[u8], palette: &Palette) -> Option<TextureImage> {
    let patch = parse_patch(bytes).ok()?;
    let w = patch.width as usize;
    let h = patch.height.max(1) as usize;
    if w == 0 {
        return None;
    }
    let mut indices: Vec<Option<u8>> = vec![None; w * h];
    for (col, posts) in patch.columns.iter().enumerate() {
        for post in posts {
            for (row, pixel) in post.pixels.iter().enumerate() {
                let y = post.top as usize + row;
                if y >= h || col >= w {
                    continue;
                }
                indices[y * w + col] = Some(*pixel);
            }
        }
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for idx in indices {
        match idx {
            Some(i) => rgba.extend_from_slice(&palette.rgba(i)),
            None => rgba.extend_from_slice(&[0, 0, 0, 0]),
        }
    }
    Some(TextureImage {
        width: patch.width,
        height: patch.height,
        rgba,
    })
}

fn compose_texture(
    def: &TextureDef,
    patches: &HashMap<usize, Patch>,
    palette: &Palette,
    missing_flag: &mut bool,
) -> TextureImage {
    let w = def.width as usize;
    let h = def.height as usize;
    let mut indices: Vec<Option<u8>> = vec![None; w * h];
    for use_def in &def.patches {
        let Some(patch) = patches.get(&(use_def.patch_index as usize)) else {
            *missing_flag = true;
            continue;
        };
        let pw = patch.width as i32;
        for col in 0..pw {
            let dst_x = use_def.x_origin as i32 + col;
            if dst_x < 0 || dst_x >= w as i32 {
                continue;
            }
            let posts = match patch.columns.get(col as usize) {
                Some(v) => v,
                None => continue,
            };
            for post in posts {
                for (row, pixel) in post.pixels.iter().enumerate() {
                    let dst_y = use_def.y_origin as i32 + post.top as i32 + row as i32;
                    if dst_y < 0 || dst_y >= h as i32 {
                        continue;
                    }
                    indices[dst_y as usize * w + dst_x as usize] = Some(*pixel);
                }
            }
        }
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for idx in indices {
        match idx {
            Some(i) => rgba.extend_from_slice(&palette.rgba(i)),
            None => rgba.extend_from_slice(&[0, 0, 0, 0]),
        }
    }
    TextureImage {
        width: def.width,
        height: def.height,
        rgba,
    }
}

fn compose_flat(bytes: &[u8], palette: &Palette) -> TextureImage {
    let mut rgba = Vec::with_capacity(FLAT_BYTES * 4);
    for &b in &bytes[..FLAT_BYTES.min(bytes.len())] {
        rgba.extend_from_slice(&palette.rgba(b));
    }
    while rgba.len() < FLAT_BYTES * 4 {
        rgba.extend_from_slice(&[0, 0, 0, 0]);
    }
    TextureImage {
        width: FLAT_WIDTH,
        height: FLAT_HEIGHT,
        rgba,
    }
}

// Re-export DirEntry usage to keep `wad::DirEntry` visible if downstream needs it.
#[allow(dead_code)]
fn _link(_: DirEntry) {}
