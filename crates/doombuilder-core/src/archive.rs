// ABOUTME: Unified opener for raw WADs, PK3s (zip), and outer zips that wrap a pk3 or wad.
// ABOUTME: Detects format from leading magic bytes; recurses one level for nested archives.

use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use std::sync::Arc;

use zip::ZipArchive;

use crate::error::{Error, Result};
use crate::wad::Wad;

const WAD_MAGIC_IWAD: &[u8; 4] = b"IWAD";
const WAD_MAGIC_PWAD: &[u8; 4] = b"PWAD";
const ZIP_MAGIC: &[u8; 4] = b"PK\x03\x04";

pub enum Asset {
    Wad(Wad),
    Pk3(Pk3),
}

pub struct Pk3 {
    archive: ZipArchive<Cursor<Arc<[u8]>>>,
}

impl Pk3 {
    pub fn from_bytes(bytes: Arc<[u8]>) -> Result<Self> {
        let archive = ZipArchive::new(Cursor::new(bytes))?;
        Ok(Self { archive })
    }

    pub fn entry_names(&self) -> Vec<String> {
        self.archive.file_names().map(|s| s.to_string()).collect()
    }

    pub fn read_entry(&mut self, name: &str) -> Result<Vec<u8>> {
        let mut entry = self.archive.by_name(name)?;
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf)?;
        Ok(buf)
    }
}

pub fn open(path: impl AsRef<Path>) -> Result<Asset> {
    let path = path.as_ref();
    let mut file = File::open(path)?;
    let mut head = [0u8; 4];
    let read = file.read(&mut head)?;

    if read >= 4 && (&head == WAD_MAGIC_IWAD || &head == WAD_MAGIC_PWAD) {
        return Ok(Asset::Wad(Wad::open(path)?));
    }
    if read >= 4 && &head == ZIP_MAGIC {
        file.rewind()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        return open_zip_bytes(Arc::from(bytes), 0);
    }
    Err(Error::NoPlayableAsset)
}

fn open_zip_bytes(bytes: Arc<[u8]>, depth: u8) -> Result<Asset> {
    if depth > 1 {
        return Err(Error::NestingTooDeep);
    }

    let cursor = Cursor::new(bytes.clone());
    let mut archive = ZipArchive::new(cursor)?;

    if has_doom_layout(&archive) {
        return Ok(Asset::Pk3(Pk3 {
            archive: ZipArchive::new(Cursor::new(bytes))?,
        }));
    }

    let inner = pick_nested_asset(&mut archive)?;
    match inner {
        NestedKind::Wad(buf) => Ok(Asset::Wad(Wad::from_bytes(buf)?)),
        NestedKind::Zip(buf) => open_zip_bytes(Arc::from(buf), depth + 1),
    }
}

enum NestedKind {
    Wad(Vec<u8>),
    Zip(Vec<u8>),
}

fn pick_nested_asset<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<NestedKind> {
    let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();
    for name in &names {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".wad") {
            let mut entry = archive.by_name(name)?;
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            return Ok(NestedKind::Wad(buf));
        }
    }
    for name in &names {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".pk3") || lower.ends_with(".zip") {
            let mut entry = archive.by_name(name)?;
            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)?;
            return Ok(NestedKind::Zip(buf));
        }
    }
    Err(Error::NoPlayableAsset)
}

/// Heuristic: a PK3 typically holds maps/, textures/, sprites/, etc., or a TEXTMAP.
/// If any top-level entry looks like a doom resource directory, treat as PK3 directly.
fn has_doom_layout<R: Read + Seek>(archive: &ZipArchive<R>) -> bool {
    archive.file_names().any(|n| {
        let lower = n.to_ascii_lowercase();
        lower.starts_with("maps/")
            || lower.starts_with("textures/")
            || lower.starts_with("sprites/")
            || lower.starts_with("flats/")
            || lower.starts_with("graphics/")
            || lower.starts_with("sounds/")
            || lower.ends_with("/textmap.txt")
            || lower == "mapinfo"
    })
}

