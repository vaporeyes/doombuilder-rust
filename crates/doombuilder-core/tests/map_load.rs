// ABOUTME: Build a synthetic single-map PWAD in memory and verify load_doom
// ABOUTME: resolves on-disk indices into stable generational IDs across slotmaps.

use doombuilder_core::wad::lumps::{
    LinedefDoom, Sector as RawSector, Sidedef as RawSidedef, ThingDoom, Vertex as RawVertex,
};
use doombuilder_core::{load_doom, Wad};

fn build_pwad(lumps: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut dir = Vec::new();
    let mut cursor = 12usize;
    for (name, data) in lumps {
        dir.push((*name, cursor as u32, data.len() as u32));
        body.extend_from_slice(data);
        cursor += data.len();
    }
    let dir_offset = cursor as u32;
    let mut out = Vec::with_capacity(12 + body.len() + dir.len() * 16);
    out.extend_from_slice(b"PWAD");
    out.extend_from_slice(&(lumps.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir_offset.to_le_bytes());
    out.extend_from_slice(&body);
    for (name, filepos, size) in dir {
        out.extend_from_slice(&filepos.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        let mut padded = [0u8; 8];
        let bytes = name.as_bytes();
        padded[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
        out.extend_from_slice(&padded);
    }
    out
}

fn pod_to_vec<T: bytemuck::Pod>(items: &[T]) -> Vec<u8> {
    bytemuck::cast_slice(items).to_vec()
}

#[test]
fn loads_minimal_doom_map() {
    // 4 vertices forming a square, 1 sector, 4 sidedefs (one per linedef, all front-only),
    // 4 linedefs, 1 player-1 thing.
    let vertexes = pod_to_vec(&[
        RawVertex { x: 0, y: 0 },
        RawVertex { x: 64, y: 0 },
        RawVertex { x: 64, y: 64 },
        RawVertex { x: 0, y: 64 },
    ]);

    let sectors = pod_to_vec(&[RawSector {
        floor_height: 0,
        ceiling_height: 128,
        floor_texture: *b"FLOOR4_8",
        ceiling_texture: *b"CEIL3_5\0",
        light: 192,
        special: 0,
        tag: 0,
    }]);

    let sd = RawSidedef {
        x_offset: 0,
        y_offset: 0,
        upper_texture: *b"-\0\0\0\0\0\0\0",
        lower_texture: *b"-\0\0\0\0\0\0\0",
        middle_texture: *b"STARTAN3",
        sector: 0,
    };
    let sidedefs = pod_to_vec(&[sd, sd, sd, sd]);

    let mk_line = |a: u16, b: u16, right: u16| LinedefDoom {
        start_vertex: a,
        end_vertex: b,
        flags: 1,
        special: 0,
        tag: 0,
        right_sidedef: right,
        left_sidedef: 0xFFFF,
    };
    let linedefs = pod_to_vec(&[
        mk_line(0, 1, 0),
        mk_line(1, 2, 1),
        mk_line(2, 3, 2),
        mk_line(3, 0, 3),
    ]);

    let things = pod_to_vec(&[ThingDoom {
        x: 32,
        y: 32,
        angle: 0,
        kind: 1,
        flags: 7,
    }]);

    let wad_bytes = build_pwad(&[
        ("MAP01", Vec::new()),
        ("THINGS", things),
        ("LINEDEFS", linedefs),
        ("SIDEDEFS", sidedefs),
        ("VERTEXES", vertexes),
        ("SECTORS", sectors),
    ]);

    let wad = Wad::from_bytes(wad_bytes).unwrap();
    let map = load_doom(&wad, "MAP01").unwrap();

    assert_eq!(map.name, "MAP01");
    assert_eq!(map.vertices.len(), 4);
    assert_eq!(map.sectors.len(), 1);
    assert_eq!(map.sidedefs.len(), 4);
    assert_eq!(map.linedefs.len(), 4);
    assert_eq!(map.things.len(), 1);

    let (sector_id, sector) = map.sectors.iter().next().unwrap();
    assert_eq!(sector.ceiling_height, 128);
    assert_eq!(sector.floor_texture.as_str(), "FLOOR4_8");
    assert_eq!(sector.sidedefs.len(), 4, "sidedef index should be rebuilt");

    for &sid in &sector.sidedefs {
        assert_eq!(map.sidedefs[sid].sector, sector_id);
    }

    for (_, line) in &map.linedefs {
        assert!(map.vertices.contains_key(line.v1));
        assert!(map.vertices.contains_key(line.v2));
        assert!(line.right.is_some());
        assert!(line.left.is_none());
    }
}

#[test]
fn missing_map_errors() {
    let wad = Wad::from_bytes(build_pwad(&[("MAP01", Vec::new())])).unwrap();
    let err = load_doom(&wad, "MAP02").unwrap_err();
    assert!(format!("{err}").contains("MAP02"));
}

#[test]
fn missing_required_lump_errors() {
    // MAP01 marker followed only by THINGS — VERTEXES missing.
    let wad = Wad::from_bytes(build_pwad(&[
        ("MAP01", Vec::new()),
        ("THINGS", vec![0u8; 10]),
    ]))
    .unwrap();
    let err = load_doom(&wad, "MAP01").unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("VERTEXES") || msg.contains("LINEDEFS"), "got: {msg}");
}
