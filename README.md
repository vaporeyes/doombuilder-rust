# doombuilder-rust

A Rust port of [Doom Builder](https://github.com/jewalky/UltimateDoomBuilder) /
[DoomBuilderX](https://github.com/Volte6/doombuilderx) targeting Doom and Hexen
map formats. The GUI is built on [iced](https://github.com/iced-rs/iced); map
data lives in generational arenas; rendering uses iced's `Canvas` for 2D and
will use a `wgpu` shader widget for 3D.

This is an early work-in-progress. The viewer is usable today; editing is not
implemented yet.

## Status

### Working

- Open `.wad`, `.pk3`, or `.zip` archives (zip-of-pk3 / zip-of-wad nesting is
  unwrapped one level)
- Memory-mapped, zero-copy WAD reader (Doom + Hexen formats)
- Map loading with format auto-detect (BEHAVIOR lump => Hexen)
- Sector polygon recovery and Constrained Delaunay triangulation via
  [`spade`](https://crates.io/crates/spade) (robust to malformed sectors;
  bad sectors are skipped, not crashed on)
- Wall quad generation for 3D mode (solid, upper-step, lower-step)
- R-tree spatial index ([`rstar`](https://crates.io/crates/rstar)) for
  O(log N) hover and click hit-testing
- 2D viewport: power-of-two adaptive grid, filled triangulated sectors,
  one-sided / two-sided linedef styling, vertex dots, hover and selection
  highlights, mouse-cursor pivot zoom, middle/right drag pan
- Vanilla Doom game config bundled (140 linedef specials, 97 thing types,
  17 sector specials, named flag bits) so the inspector shows
  "26 - DR Door (Blue) Open Wait Close" instead of "26"
- Top menu bar (File / Edit / View / Tools / Help), map picker, mode toggle
- Bottom inspector with linedef action / length / sector heights, plus
  Front Side and Back Side texture slot panels
- Status bar with element counts, current grid step, and zoom level

### Placeholder

- **3D mode** has the geometry data wired up (triangulated floors / ceilings,
  wall quads with sector light) but the wgpu render pipeline is not yet
  written; the viewport currently shows mesh counts.
- **Texture slots** show texture names only. Real pixel previews need a
  PNAMES / TEXTURE1 / TEXTURE2 / PLAYPAL compositor.

### Not implemented

- Saving (WAD or otherwise)
- UDMF read or write
- Editing operations (vertex drag, line draw, sector make, delete, etc.)
- Undo / redo
- Multi-select, rectangle-select, copy / paste
- BSP node building (REJECT, BLOCKMAP, NODES, SEGS, SSECTORS)
- Plugin system, Lua scripting
- Hexen / Boom / ZDoom / UDMF game configs (vanilla Doom only at present)
- Map analysis (orphaned lines, unclosed sectors, missing textures)

## Workspace layout

```
doombuilder-rust/
  Cargo.toml                   workspace root
  crates/
    doombuilder-core/          file I/O, map data, game configs
      configs/doom.toml        bundled vanilla Doom config
      src/
        archive.rs             unified WAD / PK3 / nested-zip opener
        config.rs              GameConfig: linedef specials, things, flags
        error.rs               single Error type
        format.rs              MapFormat { Doom, Hexen }
        map/                   slotmap-backed Map model + loader
        wad/                   zero-copy POD lump structs + directory
    doombuilder-render/        CPU-side mesh + spatial index
      src/
        loops.rs               sector edge-loop recovery
        mesh.rs                spade CDT triangulation
        spatial.rs             R-tree hit-test (vertices, lines, sectors)
        walls.rs               3D wall quad geometry
    doombuilder-gui/           iced application
      src/
        camera.rs              2D camera (world / screen, pan, zoom, frame)
        view2d.rs              Canvas viewport: grid, geometry, highlights
        lib.rs                 App, menu bar, panels, message routing
    doombuilder-app/           thin binary that calls doombuilder_gui::run
```

## Build & run

Requires a recent stable Rust (Edition 2021, MSRV is whatever iced 0.14
requires; tested with 1.95).

```bash
cargo run -p doombuilder-app
```

Then `File > Open WAD...` and pick a `.wad` file. Maps in the WAD appear in
the **Map** dropdown. Selecting a map auto-frames the viewport.

### Mouse model in the 2D viewport

| Action            | Result                                             |
|-------------------|----------------------------------------------------|
| Cursor move       | Hover highlight under the cursor                   |
| Left click        | Select the hovered element (vertex / line / sector)|
| Middle / right drag | Pan                                              |
| Scroll wheel      | Zoom about the cursor                              |

Selection priority is **vertex > linedef > sector** so corners stay
clickable on top of dense linework.

## Architecture notes

The project is intentionally split so the GPU layer can change without
disturbing the data layer.

- **`doombuilder-core`** is GUI-free. It depends only on `bytemuck`, `memmap2`,
  `zip`, `slotmap`, `serde`, `toml`, and `thiserror`. You can use it from a CLI
  or a different frontend.
- **`doombuilder-render`** is GPU-free. It produces vertex / index buffers,
  triangulations, and an R-tree index from a `Map`. It does not touch wgpu or
  iced; both 2D Canvas and the future 3D shader widget consume the same data.
- **`doombuilder-gui`** is the only crate that depends on iced.

### Map data model

All map elements live in `slotmap::SlotMap`s keyed by typed generational IDs
(`VertexId`, `LinedefId`, `SidedefId`, `SectorId`, `ThingId`). Cross-references
are by ID, never by `&` or `&mut`, so:

- Mutations are local to a single SlotMap and never invalidate other IDs.
- Deleting a vertex makes its old `VertexId` safely return `None` from
  `map.vertices.get(id)` even if a new vertex reuses the slot. No
  use-after-free, no aliasing.
- Borrow-checker fights vanish: any function that needs to walk the map can
  take a `&Map` and look elements up by ID.

`MapSector` keeps a derived `sidedefs: Vec<SidedefId>` index for fast reverse
lookup during editing. `Map::rebuild_sidedef_index()` repopulates it; it must
be called after sidedef mutations.

### Triangulation

Each sector's loops are walked from its sidedef -> linedef -> vertex graph.
T-junctions and unclosed loops are tolerated: the edges that don't stitch
into a closed loop are returned in `SectorLoops::orphan_edges` and the rest
of the sector is still triangulated.

The triangulator inserts loop edges as constraints into a
`ConstrainedDelaunayTriangulation`, then keeps inner-face triangles whose
centroid passes an even-odd point-in-polygon test against all loops. Holes
are handled implicitly. Self-intersecting linedef pairs are silently skipped
via `can_add_constraint` so malformed input never panics.

### Spatial index

Three R-trees (`vertices`, `linedefs`, `sectors`) are built per map via
`RTree::bulk_load`. Hit tests use:

- `nearest_neighbor` for vertices, then a radius check.
- AABB envelope intersection plus exact segment-distance for linedefs.
- AABB envelope intersection plus per-triangle point-in-test for sectors.

Pixel-space tolerances are converted to world-space at hit-test time
(`tolerance_world = tolerance_px / camera.zoom`) so the editor feels the same
at any zoom.

### Game config

`doom.toml` is a TOML reformat of DBX's vanilla Doom configuration. The
loader is in `core::config`. To add Hexen / Boom / ZDoom / UDMF support,
ship additional `.toml` files and let the user pick which one to load.

## Testing

```bash
cargo test --workspace
```

There are 16 tests across `core` and `render` covering WAD round-tripping,
map loading, the configuration loader, sector loop extraction,
triangulation, and the spatial index. The GUI crate has no tests yet
(the iced `Canvas::Program` traits are awkward to drive in a unit test).

## Roadmap (priority order)

1. Multi-select and rectangle-drag selection
2. First geometry mutation (vertex drag) plus undo / redo
3. Texture compositor (PALETTE -> PNAMES -> TEXTURE1/2 -> previews)
4. 3D mode wgpu pipeline (orbit camera, untextured flat shading from sector
   light)
5. WAD save
6. UDMF read and write
7. Node building (vendor [`bsp-rs`](https://github.com/sirjuddington/SLADE) /
   shell out to ZDBSP)
8. Action / texture browser dialogs
9. Hexen / Boom / ZDoom / UDMF game configs
10. Plugin / Lua extensibility

## License

Dual-licensed under either of:

- MIT License
- Apache License, Version 2.0

at your option.

## Acknowledgements

- [Doom Builder](https://github.com/jewalky/UltimateDoomBuilder) and
  [DoomBuilderX](https://github.com/Volte6/doombuilderx) for the
  configuration data and the editor UX patterns.
- [iced](https://github.com/iced-rs/iced), [spade](https://github.com/Stoeoef/spade),
  [rstar](https://github.com/georust/rstar), [slotmap](https://github.com/orlp/slotmap),
  [bytemuck](https://github.com/Lokathor/bytemuck), and the wider Rust
  ecosystem.
