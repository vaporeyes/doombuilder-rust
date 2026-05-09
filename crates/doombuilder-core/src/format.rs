// ABOUTME: Identifies which on-disk map format a WAD's map lumps use.
// ABOUTME: v0 targets vanilla Doom and Hexen; UDMF is intentionally out of scope.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapFormat {
    Doom,
    Hexen,
}
