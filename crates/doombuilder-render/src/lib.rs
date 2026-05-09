// ABOUTME: Public surface for doombuilder-render. Sector loop extraction,
// ABOUTME: triangulation, and 3D wall geometry generation live here.

pub mod loops;
pub mod mesh;
pub mod walls;

pub use loops::{extract_sector_loops, SectorLoops};
pub use mesh::{triangulate_sector, FloorMesh, MeshError};
pub use walls::{build_walls, Wall, WallKind};
