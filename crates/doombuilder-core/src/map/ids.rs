// ABOUTME: Generational keys for the map's slotmaps. Each ID is opaque,
// ABOUTME: stable across other elements' insertions and deletions, and O(1) to look up.

use slotmap::new_key_type;

new_key_type! {
    pub struct VertexId;
    pub struct LinedefId;
    pub struct SidedefId;
    pub struct SectorId;
    pub struct ThingId;
}
