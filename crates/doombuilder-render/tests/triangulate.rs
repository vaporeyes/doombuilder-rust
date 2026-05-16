// ABOUTME: Verify loop extraction and triangulation against a synthetic
// ABOUTME: single-sector square map. Real-WAD coverage comes from manual smoke tests.

use doombuilder_core::map::{Map, MapLinedef, MapSector, MapSidedef, MapVertex, TextureName};
use doombuilder_core::MapFormat;
use doombuilder_render::{extract_sector_loops, triangulate_sector};

fn square_map() -> Map {
    let mut map = Map::new("TEST", MapFormat::Doom);
    let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
    let v1 = map.vertices.insert(MapVertex { x: 64, y: 0 });
    let v2 = map.vertices.insert(MapVertex { x: 64, y: 64 });
    let v3 = map.vertices.insert(MapVertex { x: 0, y: 64 });

    let sec = map.sectors.insert(MapSector {
        floor_height: 0,
        ceiling_height: 128,
        floor_texture: TextureName(*b"FLAT1\0\0\0"),
        ceiling_texture: TextureName(*b"FLAT2\0\0\0"),
        light: 192,
        special: 0,
        tag: 0,
        sidedefs: Vec::new(),
        fields: Default::default(),
    });

    let make_side = |map: &mut Map, sec| {
        map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"WALL\0\0\0\0"),
        })
    };

    let s0 = make_side(&mut map, sec);
    let s1 = make_side(&mut map, sec);
    let s2 = make_side(&mut map, sec);
    let s3 = make_side(&mut map, sec);

    let mk_line = |v_a, v_b, side| MapLinedef {
        v1: v_a,
        v2: v_b,
        flags: 0,
        special: 0,
        args: [0; 5],
        tag: 0,
        right: Some(side),
        left: None,
        fields: Default::default(),
    };
    map.linedefs.insert(mk_line(v0, v1, s0));
    map.linedefs.insert(mk_line(v1, v2, s1));
    map.linedefs.insert(mk_line(v2, v3, s2));
    map.linedefs.insert(mk_line(v3, v0, s3));

    map.rebuild_sidedef_index();
    map
}

#[test]
fn extracts_single_square_loop() {
    let map = square_map();
    let by_sector = extract_sector_loops(&map);
    assert_eq!(by_sector.len(), 1);
    let (_, loops) = by_sector.iter().next().unwrap();
    assert_eq!(loops.loops.len(), 1);
    assert_eq!(loops.loops[0].len(), 4);
    assert!(loops.orphan_edges.is_empty());
}

#[test]
fn triangulates_square_to_two_triangles() {
    let map = square_map();
    let by_sector = extract_sector_loops(&map);
    let (sid, loops) = by_sector.iter().next().unwrap();
    let mesh = triangulate_sector(&map, *sid, loops).unwrap();

    assert_eq!(mesh.positions.len(), 4);
    assert_eq!(mesh.indices.len(), 6, "two triangles -> 6 indices");

    // All triangle vertices must be valid indices into positions.
    for &i in &mesh.indices {
        assert!((i as usize) < mesh.positions.len());
    }
}
