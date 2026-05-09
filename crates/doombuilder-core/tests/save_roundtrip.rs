// ABOUTME: Save a Map to PWAD bytes, parse them back, and verify counts plus
// ABOUTME: a few specific field values survive the round trip.

use doombuilder_core::map::{
    save_map_as_pwad, MapLinedef, MapSector, MapSidedef, MapVertex, TextureName,
};
use doombuilder_core::{load_doom, MapFormat, Map, Wad};

fn synthetic_doom_map() -> Map {
    let mut map = Map::new("MAP01", MapFormat::Doom);
    let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
    let v1 = map.vertices.insert(MapVertex { x: 64, y: 0 });
    let v2 = map.vertices.insert(MapVertex { x: 64, y: 64 });
    let v3 = map.vertices.insert(MapVertex { x: 0, y: 64 });
    let sec = map.sectors.insert(MapSector {
        floor_height: 0,
        ceiling_height: 128,
        floor_texture: TextureName(*b"FLAT5_5\0"),
        ceiling_texture: TextureName(*b"CEIL3_5\0"),
        light: 192,
        special: 9,
        tag: 7,
        sidedefs: Vec::new(),
    });
    let mk_side = |map: &mut Map| {
        map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 1,
            y_offset: 2,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName(*b"STARTAN3"),
        })
    };
    let s0 = mk_side(&mut map);
    let s1 = mk_side(&mut map);
    let s2 = mk_side(&mut map);
    let s3 = mk_side(&mut map);
    let mk_line = |a, b, side| MapLinedef {
        v1: a,
        v2: b,
        flags: 1,
        special: 26,
        args: [0; 5],
        tag: 11,
        right: Some(side),
        left: None,
    };
    map.linedefs.insert(mk_line(v0, v1, s0));
    map.linedefs.insert(mk_line(v1, v2, s1));
    map.linedefs.insert(mk_line(v2, v3, s2));
    map.linedefs.insert(mk_line(v3, v0, s3));
    map.rebuild_sidedef_index();
    map
}

#[test]
fn save_then_load_preserves_counts_and_fields() {
    let original = synthetic_doom_map();
    let bytes = save_map_as_pwad(&original);

    let wad = Wad::from_bytes(bytes).unwrap();
    let parsed = load_doom(&wad, "MAP01").unwrap();

    assert_eq!(parsed.vertices.len(), original.vertices.len());
    assert_eq!(parsed.linedefs.len(), original.linedefs.len());
    assert_eq!(parsed.sidedefs.len(), original.sidedefs.len());
    assert_eq!(parsed.sectors.len(), original.sectors.len());
    assert_eq!(parsed.things.len(), original.things.len());

    let (_, sector) = parsed.sectors.iter().next().unwrap();
    assert_eq!(sector.floor_height, 0);
    assert_eq!(sector.ceiling_height, 128);
    assert_eq!(sector.light, 192);
    assert_eq!(sector.special, 9);
    assert_eq!(sector.tag, 7);

    let (_, line) = parsed.linedefs.iter().next().unwrap();
    assert_eq!(line.flags, 1);
    assert_eq!(line.special, 26);
    assert_eq!(line.tag, 11);
    assert!(line.right.is_some());
    assert!(line.left.is_none());

    // Vertices must have been re-indexed but coordinates preserved.
    let mut xs: Vec<i32> = parsed.vertices.iter().map(|(_, v)| v.x).collect();
    xs.sort();
    assert_eq!(xs, vec![0, 0, 64, 64]);
}

#[test]
fn pwad_magic_is_correct() {
    let map = synthetic_doom_map();
    let bytes = save_map_as_pwad(&map);
    assert_eq!(&bytes[0..4], b"PWAD");
}
