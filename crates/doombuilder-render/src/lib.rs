// ABOUTME: Public surface for doombuilder-render. Sector loop extraction,
// ABOUTME: triangulation, and 3D wall geometry generation live here.

pub mod loops;
pub mod mesh;
pub mod sector_fill;
pub mod spatial;
pub mod walls;

pub use loops::{extract_sector_loops, SectorLoops};
pub use mesh::{triangulate_sector, FloorMesh, MeshError};
pub use sector_fill::{
    rasterise_sector_fill, rasterise_sector_fill_slot, rasterise_sector_solid, FillSlot,
    SectorFill,
};
pub use spatial::{Hit, SpatialIndex};
pub use walls::{build_walls, Wall, WallKind};
