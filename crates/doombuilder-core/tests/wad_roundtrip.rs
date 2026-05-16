// ABOUTME: Round-trip tests: build a synthetic in-memory WAD, parse it back,
// ABOUTME: and verify zero-copy slice access matches the source bytes.

use doombuilder_core::wad::lumps::{LinedefDoom, Vertex};
use doombuilder_core::wad::WadKind;
use doombuilder_core::Wad;

fn build_wad(lumps: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let header_size = 12usize;
    let entry_size = 16usize;

    let mut body = Vec::new();
    let mut directory = Vec::new();
    let mut cursor = header_size;
    for (name, data) in lumps {
        directory.push((*name, cursor as u32, data.len() as u32));
        body.extend_from_slice(data);
        cursor += data.len();
    }

    let dir_offset = cursor as u32;
    let mut out = Vec::with_capacity(header_size + body.len() + directory.len() * entry_size);
    out.extend_from_slice(b"PWAD");
    out.extend_from_slice(&(lumps.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir_offset.to_le_bytes());
    out.extend_from_slice(&body);
    for (name, filepos, size) in directory {
        out.extend_from_slice(&filepos.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        let mut padded = [0u8; 8];
        let bytes = name.as_bytes();
        padded[..bytes.len().min(8)].copy_from_slice(&bytes[..bytes.len().min(8)]);
        out.extend_from_slice(&padded);
    }
    out
}

#[test]
fn parses_pwad_header_and_directory() {
    let bytes = build_wad(&[("VERTEXES", vec![0; 16]), ("LINEDEFS", vec![0; 28])]);
    let wad = Wad::from_bytes(bytes).unwrap();
    assert_eq!(wad.kind(), WadKind::Pwad);
    assert_eq!(wad.directory().len(), 2);
    assert_eq!(wad.directory()[0].name_str(), "VERTEXES");
    assert_eq!(wad.directory()[1].name_str(), "LINEDEFS");
}

#[test]
fn vertices_zero_copy() {
    let mut data = Vec::new();
    for (x, y) in [(0i16, 0i16), (64, 0), (64, 128), (0, 128)] {
        data.extend_from_slice(&x.to_le_bytes());
        data.extend_from_slice(&y.to_le_bytes());
    }
    let bytes = build_wad(&[("VERTEXES", data)]);
    let wad = Wad::from_bytes(bytes).unwrap();
    let entry = wad.find("VERTEXES").unwrap();
    let verts: &[Vertex] = wad.lump_as_slice(entry).unwrap();
    assert_eq!(verts.len(), 4);
    assert_eq!(verts[2].x, 64);
    assert_eq!(verts[2].y, 128);
}

#[test]
fn linedefs_doom_zero_copy() {
    let line = LinedefDoom {
        start_vertex: 0,
        end_vertex: 1,
        flags: 1,
        special: 0,
        tag: 0,
        right_sidedef: 0,
        left_sidedef: 0xFFFF,
    };
    let raw = bytemuck::bytes_of(&line).to_vec();
    let bytes = build_wad(&[("LINEDEFS", raw)]);
    let wad = Wad::from_bytes(bytes).unwrap();
    let entry = wad.find("LINEDEFS").unwrap();
    let lines: &[LinedefDoom] = wad.lump_as_slice(entry).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].end_vertex, 1);
    assert_eq!(lines[0].left_sidedef, 0xFFFF);
}

#[test]
fn rejects_bad_magic() {
    let mut bytes = vec![0u8; 12];
    bytes[..4].copy_from_slice(b"XWAD");
    let err = Wad::from_bytes(bytes).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("bad magic"), "got: {msg}");
}
