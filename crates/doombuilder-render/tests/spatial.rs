// ABOUTME: Verify the R-tree spatial index returns the expected hit for points
// ABOUTME: hovering near vertices, on linedefs, and inside the sector polygon.

use doombuilder_core::map::{
    Map, MapLinedef, MapSector, MapSidedef, MapVertex, TextureName, VertexId,
};
use doombuilder_core::MapFormat;
use doombuilder_render::{extract_sector_loops, triangulate_sector, Hit, SpatialIndex};

fn square_map() -> (Map, [VertexId; 4]) {
    let mut map = Map::new("TEST", MapFormat::Doom);
    let v0 = map.vertices.insert(MapVertex { x: 0, y: 0 });
    let v1 = map.vertices.insert(MapVertex { x: 64, y: 0 });
    let v2 = map.vertices.insert(MapVertex { x: 64, y: 64 });
    let v3 = map.vertices.insert(MapVertex { x: 0, y: 64 });

    let sec = map.sectors.insert(MapSector {
        floor_height: 0,
        ceiling_height: 128,
        floor_texture: TextureName([0; 8]),
        ceiling_texture: TextureName([0; 8]),
        light: 192,
        special: 0,
        tag: 0,
        sidedefs: Vec::new(),
        fields: Default::default(),
    });
    let mk_side = |map: &mut Map| {
        map.sidedefs.insert(MapSidedef {
            sector: sec,
            x_offset: 0,
            y_offset: 0,
            upper_texture: TextureName([0; 8]),
            lower_texture: TextureName([0; 8]),
            middle_texture: TextureName([0; 8]),
        })
    };
    let s0 = mk_side(&mut map);
    let s1 = mk_side(&mut map);
    let s2 = mk_side(&mut map);
    let s3 = mk_side(&mut map);
    let mk_line = |a, b, side| MapLinedef {
        v1: a,
        v2: b,
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
    (map, [v0, v1, v2, v3])
}

fn build_index(map: &Map) -> SpatialIndex {
    let loops = extract_sector_loops(map);
    let meshes: Vec<_> = loops
        .iter()
        .filter_map(|(sid, l)| triangulate_sector(map, *sid, l).ok().map(|m| (*sid, m)))
        .collect();
    SpatialIndex::build(map, meshes)
}

#[test]
fn nearest_vertex_within_radius() {
    let (map, ids) = square_map();
    let idx = build_index(&map);
    let hit = idx.nearest_vertex(2.0, 1.0, 4.0).expect("vertex hit");
    assert_eq!(hit, ids[0]);
}

#[test]
fn no_vertex_when_outside_radius() {
    let (map, _) = square_map();
    let idx = build_index(&map);
    assert!(idx.nearest_vertex(32.0, 32.0, 4.0).is_none());
}

#[test]
fn linedef_hit_at_midpoint() {
    let (map, _) = square_map();
    let idx = build_index(&map);
    // (32, 0) is the midpoint of the bottom edge.
    assert!(idx.nearest_linedef(32.0, 0.5, 2.0).is_some());
}

#[test]
fn sector_hit_inside_polygon() {
    let (map, _) = square_map();
    let idx = build_index(&map);
    assert!(idx.sector_at(32.0, 32.0).is_some());
    assert!(idx.sector_at(-10.0, -10.0).is_none());
}

#[test]
fn vertices_in_rect_returns_contained_only() {
    let (map, ids) = square_map();
    let idx = build_index(&map);
    let inside = idx.vertices_in_rect([-1.0, -1.0], [10.0, 10.0]);
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0], ids[0]);

    let all = idx.vertices_in_rect([-1.0, -1.0], [65.0, 65.0]);
    assert_eq!(all.len(), 4);
}

#[test]
fn linedefs_in_rect_requires_both_endpoints() {
    let (map, _) = square_map();
    let idx = build_index(&map);
    // Only the bottom edge has both endpoints in this rect.
    let lines = idx.linedefs_in_rect([-1.0, -1.0], [65.0, 1.0]);
    assert_eq!(lines.len(), 1);
    // Whole map: 4 linedefs.
    let all = idx.linedefs_in_rect([-1.0, -1.0], [65.0, 65.0]);
    assert_eq!(all.len(), 4);
    // Box that only clips one endpoint of any line: 0.
    let none = idx.linedefs_in_rect([-100.0, -100.0], [-1.0, -1.0]);
    assert_eq!(none.len(), 0);
}

#[test]
fn hit_priority_vertex_over_linedef_over_sector() {
    let (map, ids) = square_map();
    let idx = build_index(&map);
    // Right at corner (0,0): vertex wins.
    match idx.hit_test(0.5, 0.5, 4.0, 4.0) {
        Some(Hit::Vertex(v)) => assert_eq!(v, ids[0]),
        other => panic!("expected vertex hit, got {other:?}"),
    }
    // Just inside the bottom edge: linedef wins over sector.
    assert!(matches!(
        idx.hit_test(32.0, 0.5, 2.0, 2.0),
        Some(Hit::Linedef(_))
    ));
    // Deep in the middle: sector.
    assert!(matches!(
        idx.hit_test(32.0, 32.0, 2.0, 2.0),
        Some(Hit::Sector(_))
    ));
}
