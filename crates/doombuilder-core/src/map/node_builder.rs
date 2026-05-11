// ABOUTME: Pluggable BSP/blockmap/reject builder. Default is the in-house
// ABOUTME: `nodes::build_nodes`; alternatively shells out to zdbsp.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Error, Result};
use crate::map::{nodes, Map, NodeOutput};
use crate::wad::Wad;

/// Selects which implementation produces the derived BSP/blockmap/reject lumps.
#[derive(Debug, Clone, Default)]
pub enum NodeBuilder {
    /// Use the bundled Rust node builder. No external dependencies.
    #[default]
    Builtin,
    /// Shell out to a zdbsp executable. The map is written to a temp PWAD,
    /// zdbsp is run, and the BSP lumps are pulled back out of its output.
    Zdbsp {
        /// Path to (or bare name of) the zdbsp binary. A bare name resolves
        /// against `PATH`; an absolute or relative path is used verbatim.
        exe: PathBuf,
        /// Extra command-line flags to pass before the input filename.
        /// Empty for the default zdbsp behavior.
        extra_args: Vec<OsString>,
    },
}

/// Run the selected builder against `map`. The returned `NodeOutput` is
/// drop-in compatible with what `nodes::build_nodes` produces.
pub fn build(map: &Map, builder: &NodeBuilder) -> Result<NodeOutput> {
    match builder {
        NodeBuilder::Builtin => Ok(nodes::build_nodes(map)),
        NodeBuilder::Zdbsp { exe, extra_args } => build_with_zdbsp(map, exe, extra_args),
    }
}

fn build_with_zdbsp(map: &Map, exe: &Path, extra_args: &[OsString]) -> Result<NodeOutput> {
    // Build a PWAD containing the map's source lumps. zdbsp ignores any
    // existing SEGS/SSECTORS/NODES/REJECT/BLOCKMAP and writes its own, so we
    // include empty placeholders for shape only.
    let input_pwad = serialize_for_external_builder(map);

    let dir = TempDir::new("doombuilder-zdbsp")?;
    let in_path = dir.path().join("in.wad");
    let out_path = dir.path().join("out.wad");
    std::fs::write(&in_path, &input_pwad)?;

    let status = Command::new(exe)
        .args(extra_args)
        .arg("-o")
        .arg(&out_path)
        .arg(&in_path)
        .status()
        .map_err(|e| Error::Io(std::io::Error::new(
            e.kind(),
            format!("failed to spawn zdbsp ({}): {e}", exe.display()),
        )))?;
    if !status.success() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("zdbsp exited with status {status}"),
        )));
    }

    let bytes = std::fs::read(&out_path)?;
    let wad = Wad::from_bytes(bytes)?;
    extract_node_output(&wad, &map.name, map.vertices.len() as u32)
}

/// Build a minimal PWAD with just the map's source lumps. Mirrors
/// `serialize_map` but skips BSP-derived lumps the external builder will
/// generate. Kept private to avoid widening the public API.
fn serialize_for_external_builder(map: &Map) -> Vec<u8> {
    use crate::wad::{write_pwad, LumpEntry};
    use std::collections::HashMap;

    let vertex_idx: HashMap<_, u16> = map
        .vertices
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();
    let sidedef_idx: HashMap<_, u16> = map
        .sidedefs
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();
    let sector_idx: HashMap<_, u16> = map
        .sectors
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id, i as u16))
        .collect();

    let things = crate::map::save::serialize_things(map);
    let linedefs = crate::map::save::serialize_linedefs(map, &vertex_idx, &sidedef_idx);
    let sidedefs = crate::map::save::serialize_sidedefs(map, &sector_idx);
    let vertexes = crate::map::save::serialize_vertexes(map);
    let sectors = crate::map::save::serialize_sectors(map);

    let mut lumps: Vec<LumpEntry<'_>> = vec![
        LumpEntry { name: &map.name, data: Vec::new() },
        LumpEntry { name: "THINGS", data: things },
        LumpEntry { name: "LINEDEFS", data: linedefs },
        LumpEntry { name: "SIDEDEFS", data: sidedefs },
        LumpEntry { name: "VERTEXES", data: vertexes },
        LumpEntry { name: "SECTORS", data: sectors },
    ];
    if map.format == crate::format::MapFormat::Hexen {
        lumps.push(LumpEntry { name: "BEHAVIOR", data: Vec::new() });
    }
    write_pwad(&lumps)
}

/// Pull SEGS/SSECTORS/NODES/REJECT/BLOCKMAP plus any vertices zdbsp added
/// past the map's original count, packaged the way our pipeline expects.
fn extract_node_output(wad: &Wad, map_name: &str, base_vertex_count: u32) -> Result<NodeOutput> {
    let dir = wad.directory();
    let map_idx = dir
        .iter()
        .position(|e| e.name_str().eq_ignore_ascii_case(map_name))
        .ok_or_else(|| Error::MapNotFound(map_name.to_string()))?;

    // zdbsp may emit GL_VERT / GL_SEGS / GL_SSECT / GL_NODES alongside the
    // classic ones; we only consume the classic lumps. Search forward from
    // the map marker until we hit the next map marker (THINGS-followed lump).
    let scan_end = (map_idx + 1..dir.len())
        .find(|&i| {
            // A new map marker is any entry whose next sibling is THINGS.
            dir.get(i + 1)
                .map(|n| n.name_str().eq_ignore_ascii_case("THINGS"))
                .unwrap_or(false)
        })
        .unwrap_or(dir.len());

    let lump_named = |name: &str| -> Result<Vec<u8>> {
        for i in (map_idx + 1)..scan_end {
            if dir[i].name_str().eq_ignore_ascii_case(name) {
                return Ok(wad.lump_bytes(&dir[i])?.to_vec());
            }
        }
        Err(Error::MissingMapLump {
            map: map_name.to_string(),
            lump: name.to_string(),
        })
    };

    let segs = lump_named("SEGS")?;
    let ssectors = lump_named("SSECTORS")?;
    let nodes = lump_named("NODES")?;
    let reject = lump_named("REJECT")?;
    let blockmap = lump_named("BLOCKMAP")?;

    // Recover the extra vertices zdbsp appended (one (i16, i16) pair per slot
    // past `base_vertex_count`). If the output VERTEXES lump is shorter than
    // expected (shouldn't happen but guard anyway), treat as no extras.
    let vertexes = lump_named("VERTEXES")?;
    let total_vertices = (vertexes.len() / 4) as u32;
    let mut extra_vertices = Vec::new();
    if total_vertices > base_vertex_count {
        for i in base_vertex_count..total_vertices {
            let off = (i as usize) * 4;
            let x = i16::from_le_bytes([vertexes[off], vertexes[off + 1]]);
            let y = i16::from_le_bytes([vertexes[off + 2], vertexes[off + 3]]);
            extra_vertices.push((x, y));
        }
    }

    Ok(NodeOutput {
        extra_vertices,
        segs,
        ssectors,
        nodes,
        blockmap,
        reject,
    })
}

/// Self-cleaning temporary directory. We avoid the `tempfile` crate dep for
/// such a small need; the directory is removed on drop, errors swallowed
/// because temp cleanup must not mask the real result.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> std::io::Result<Self> {
        // Combine PID + nanosecond timestamp + an atomic counter so concurrent
        // builds don't collide. UUID would be nicer but adds a dep.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "{}-{}-{}-{}",
            prefix,
            std::process::id(),
            ts,
            n
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
