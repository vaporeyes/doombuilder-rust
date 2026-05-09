// ABOUTME: Fixed-layout POD structs for Doom and Hexen map lumps.
// ABOUTME: All structs are repr(C) with no padding so bytemuck can zero-copy cast.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub x: i16,
    pub y: i16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct LinedefDoom {
    pub start_vertex: u16,
    pub end_vertex: u16,
    pub flags: u16,
    pub special: u16,
    pub tag: u16,
    pub right_sidedef: u16,
    pub left_sidedef: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct LinedefHexen {
    pub start_vertex: u16,
    pub end_vertex: u16,
    pub flags: u16,
    pub special: u8,
    pub args: [u8; 5],
    pub right_sidedef: u16,
    pub left_sidedef: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Sidedef {
    pub x_offset: i16,
    pub y_offset: i16,
    pub upper_texture: [u8; 8],
    pub lower_texture: [u8; 8],
    pub middle_texture: [u8; 8],
    pub sector: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Sector {
    pub floor_height: i16,
    pub ceiling_height: i16,
    pub floor_texture: [u8; 8],
    pub ceiling_texture: [u8; 8],
    pub light: i16,
    pub special: u16,
    pub tag: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ThingDoom {
    pub x: i16,
    pub y: i16,
    pub angle: u16,
    pub kind: u16,
    pub flags: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct ThingHexen {
    pub tid: u16,
    pub x: i16,
    pub y: i16,
    pub z: i16,
    pub angle: u16,
    pub kind: u16,
    pub flags: u16,
    pub special: u8,
    pub args: [u8; 5],
}

#[cfg(test)]
mod size_checks {
    use super::*;

    const _: () = {
        assert!(std::mem::size_of::<Vertex>() == 4);
        assert!(std::mem::size_of::<LinedefDoom>() == 14);
        assert!(std::mem::size_of::<LinedefHexen>() == 16);
        assert!(std::mem::size_of::<Sidedef>() == 30);
        assert!(std::mem::size_of::<Sector>() == 26);
        assert!(std::mem::size_of::<ThingDoom>() == 10);
        assert!(std::mem::size_of::<ThingHexen>() == 20);
    };
}
