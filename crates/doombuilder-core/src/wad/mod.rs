// ABOUTME: WAD reader: opens a backing buffer (mmap or owned bytes) and exposes
// ABOUTME: the header, directory, and zero-copy slice access into named lumps.

mod directory;
mod header;
pub mod lumps;
pub mod write;

pub use write::{write_pwad, LumpEntry};

pub use directory::DirEntry;
pub use header::{WadHeader, WadKind};

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use bytemuck::Pod;
use memmap2::Mmap;

use crate::error::{Error, Result};

#[derive(Clone)]
enum Backing {
    Mmap(Arc<Mmap>),
    Owned(Arc<[u8]>),
}

impl Backing {
    fn bytes(&self) -> &[u8] {
        match self {
            Backing::Mmap(m) => &m[..],
            Backing::Owned(b) => &b[..],
        }
    }
}

#[derive(Clone)]
pub struct Wad {
    backing: Backing,
    header: WadHeader,
    directory: Vec<DirEntry>,
}

impl std::fmt::Debug for Wad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wad")
            .field("kind", &self.header.kind)
            .field("num_lumps", &self.header.num_lumps)
            .field("bytes", &self.backing.bytes().len())
            .finish()
    }
}

impl Wad {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        Self::from_backing(Backing::Mmap(Arc::new(mmap)))
    }

    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Result<Self> {
        Self::from_backing(Backing::Owned(bytes.into()))
    }

    fn from_backing(backing: Backing) -> Result<Self> {
        let bytes = backing.bytes();
        let header = WadHeader::parse(bytes)?;
        let directory = directory::parse(bytes, &header)?;
        Ok(Self {
            backing,
            header,
            directory,
        })
    }

    pub fn header(&self) -> &WadHeader {
        &self.header
    }

    pub fn kind(&self) -> WadKind {
        self.header.kind
    }

    pub fn directory(&self) -> &[DirEntry] {
        &self.directory
    }

    pub fn bytes(&self) -> &[u8] {
        self.backing.bytes()
    }

    /// Raw bytes for an entry, without interpretation.
    pub fn lump_bytes(&self, entry: &DirEntry) -> Result<&[u8]> {
        let offset = entry.filepos as usize;
        let size = entry.size as usize;
        let bytes = self.backing.bytes();
        let end = offset
            .checked_add(size)
            .ok_or_else(|| Error::Truncated {
                offset: offset as u64,
                expected: size as u64,
                actual: bytes.len() as u64,
            })?;
        if end > bytes.len() {
            return Err(Error::Truncated {
                offset: offset as u64,
                expected: size as u64,
                actual: bytes.len() as u64,
            });
        }
        Ok(&bytes[offset..end])
    }

    /// Zero-copy view of a lump as a slice of `T` (a fixed-layout POD struct).
    /// Returns an error if the lump size is not a multiple of `size_of::<T>()`
    /// or the lump start offset is not aligned for `T`.
    pub fn lump_as_slice<T: Pod>(&self, entry: &DirEntry) -> Result<&[T]> {
        let bytes = self.lump_bytes(entry)?;
        let stride = std::mem::size_of::<T>();
        if stride == 0 || bytes.len() % stride != 0 {
            return Err(Error::BadLumpSize {
                name: entry.name_str().to_string(),
                size: bytes.len(),
                stride,
            });
        }
        bytemuck::try_cast_slice(bytes).map_err(|_| Error::Unaligned {
            name: entry.name_str().to_string(),
            offset: entry.filepos as usize,
            align: std::mem::align_of::<T>(),
        })
    }

    /// Owned copy of a lump as `Vec<T>`. Use when the lump may sit at an
    /// unaligned file offset (common in PWADs from Doom-era editors that
    /// don't pad lumps), or when the caller plans to own the data anyway.
    /// Falls back to a per-element memcpy if zero-copy isn't possible.
    pub fn lump_to_vec<T: Pod>(&self, entry: &DirEntry) -> Result<Vec<T>> {
        let bytes = self.lump_bytes(entry)?;
        let stride = std::mem::size_of::<T>();
        if stride == 0 || bytes.len() % stride != 0 {
            return Err(Error::BadLumpSize {
                name: entry.name_str().to_string(),
                size: bytes.len(),
                stride,
            });
        }
        let count = bytes.len() / stride;
        let mut out: Vec<T> = Vec::with_capacity(count);
        // Fast path: properly aligned, just copy the slice.
        if let Ok(slice) = bytemuck::try_cast_slice::<u8, T>(bytes) {
            out.extend_from_slice(slice);
            return Ok(out);
        }
        // Unaligned path: copy each record into a freshly aligned slot.
        // SAFETY: `T: Pod` ⇒ any byte pattern of the right size is a valid
        // value; we copy exactly `stride` bytes into each MaybeUninit<T>.
        for i in 0..count {
            let src = &bytes[i * stride..(i + 1) * stride];
            let mut v = std::mem::MaybeUninit::<T>::uninit();
            unsafe {
                std::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    v.as_mut_ptr() as *mut u8,
                    stride,
                );
                out.push(v.assume_init());
            }
        }
        Ok(out)
    }

    pub fn find(&self, name: &str) -> Option<&DirEntry> {
        self.directory.iter().find(|e| e.name_str() == name)
    }

    /// Names of map markers — lumps whose next sibling is a recognised map
    /// data lump such as THINGS or TEXTMAP. We do NOT require the marker to
    /// be size 0: Hexen IWAD map markers carry 12 bytes of metadata, and
    /// some custom WADs use non-empty markers too. The "next is THINGS"
    /// constraint is enough to disambiguate.
    pub fn map_markers(&self) -> Vec<String> {
        const FOLLOWS: &[&str] = &["THINGS", "TEXTMAP"];
        let mut out = Vec::new();
        let dir = &self.directory;
        for i in 0..dir.len().saturating_sub(1) {
            let next = dir[i + 1].name_str();
            if FOLLOWS.iter().any(|n| n.eq_ignore_ascii_case(next)) {
                out.push(dir[i].name_str().to_string());
            }
        }
        out
    }
}
