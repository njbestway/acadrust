//! Example: write DWG files containing a 3DSOLID box with valid ACIS data.
//!
//! Builds a 10×10×10 box centered at the origin using the SAT builder API,
//! then writes it to **five** DWG files:
//!
//! - `solid3d_empty_r2000.dwg` — R2000 (AC1015): empty 3DSOLID (no ACIS data)
//! - `solid3d_mini_r2000.dwg`  — R2000 (AC1015): minimal SAT body
//! - `solid3d_r2000.dwg`       — R2000 (AC1015): full box, SAT text
//! - `solid3d_r2004.dwg`       — R2004 (AC1018): full box, SAT text (selective cipher)
//! - `solid3d_r2013.dwg`       — R2013 (AC1027): full box, SAB binary
//!
//! ```
//! cargo run --example write_3dsolid_dwg
//! ```

use acadrust::{CadDocument, DwgWriter, DxfVersion, EntityType};
use acadrust::entities::Solid3D;
use acadrust::entities::acis::{SabWriter, SatDocument, SatPointer, SatToken, Sense, Sidedness};

fn main() -> acadrust::Result<()> {
    // ── 1. Build the SAT document describing a 10×10×10 box ──────────
    let sat = build_box_sat();

    // Print the generated SAT text for inspection
    let sat_text = sat.to_sat_string();
    println!("=== Generated SAT data ({} bytes) ===", sat_text.len());
    for (i, line) in sat_text.lines().enumerate() {
        println!("  {:>2}: {}", i, line);
    }

    // Validate the document
    let errors = sat.validate();
    if !errors.is_empty() {
        println!("\nSAT validation warnings ({}):", errors.len());
        for e in &errors {
            println!("  - {:?}", e);
        }
    }

    // ── 2a. Write EMPTY 3DSOLID (R2000) — no ACIS data ─────────────
    //    Tests whether entity structure (_unknown bit + history handle)
    //    is correct when no modeler data is present.
    {
        let solid = Solid3D::new(); // empty, acis_empty = true
        let version = DxfVersion::AC1015;
        let mut doc = CadDocument::with_version(version);
        doc.add_entity(EntityType::Solid3D(solid))?;

        let path = "solid3d_empty_r2000.dwg";
        DwgWriter::write_to_file(path, &doc)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("\n[DIAG] Written: {} ({} bytes) — empty 3DSOLID, R2000", path, size);
    }

    // ── 2b. Write MINIMAL SAT 3DSOLID (R2000) — simple body ────────
    //    Uses a hand-crafted SAT string with minimal valid geometry
    //    (body → lump → shell → face → loop → coedge → edge → vertex)
    //    to test whether SAT text format is correctly encoded.
    {
        let minimal_sat = "\
700 0 1 0\n\
@8 acadrust @8 ACIS 7.0 @24 Thu Jan 01 00:00:00 2023\n\
10 9.9999999999999995e-007 1e-010\n\
body $-1 -1 $-1 $1 $-1 $-1 #\n\
lump $-1 -1 $-1 $-1 $2 $0 #\n\
shell $-1 -1 $-1 $-1 $-1 $3 $-1 $1 #\n\
face $-1 -1 $-1 $-1 $4 $2 $-1 $5 forward single #\n\
loop $-1 -1 $-1 $-1 $6 $3 #\n\
plane-surface $-1 -1 $-1 0 0 5 0 0 1 1 0 0 forward_v I I I I #\n\
coedge $-1 -1 $-1 $6 $6 $-1 $7 forward $4 $-1 #\n\
edge $-1 -1 $-1 $8 0 $8 1 $6 $9 forward #\n\
vertex $-1 -1 $-1 $7 $10 #\n\
straight-curve $-1 -1 $-1 -5 -5 5 1 0 0 I I #\n\
point $-1 -1 $-1 -5 -5 5 #\n\
End-of-ACIS-data\n";

        let mut solid = Solid3D::from_sat(minimal_sat);
        solid.common.layer = "0".to_string();

        let version = DxfVersion::AC1015;
        let mut doc = CadDocument::with_version(version);
        doc.add_entity(EntityType::Solid3D(solid))?;

        let path = "solid3d_mini_r2000.dwg";
        DwgWriter::write_to_file(path, &doc)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("[DIAG] Written: {} ({} bytes) — minimal SAT, R2000", path, size);
    }

    // ── 2c. Write full box (R2000 — text lines, no encryption) ──────
    {
        let mut solid = Solid3D::new();
        solid.set_sat_document(&sat);
        solid.common.layer = "0".to_string();

        let version = DxfVersion::AC1015; // R2000
        let mut doc = CadDocument::with_version(version);
        doc.add_entity(EntityType::Solid3D(solid))?;

        let path = "solid3d_r2000.dwg";
        DwgWriter::write_to_file(path, &doc)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("[R2000] Written: {} ({} bytes, version: {:?})", path, size, version);
    }

    // ── 3. Write SAT version (R2004 — encrypted SAT text) ───────────
    {
        let mut solid = Solid3D::new();
        solid.set_sat_document(&sat);
        solid.common.layer = "0".to_string();

        let version = DxfVersion::AC1018; // R2004
        let mut doc = CadDocument::with_version(version);
        doc.add_entity(EntityType::Solid3D(solid))?;

        let path = "solid3d_r2004.dwg";
        DwgWriter::write_to_file(path, &doc)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("[R2004] Written: {} ({} bytes, version: {:?})", path, size, version);
    }

    // ── 4. Write SAB version (R2013 — SAB binary) ───────────────────
    {
        let sab_data = SabWriter::write(&sat);
        println!("[SAB] Generated SAB binary: {} bytes", sab_data.len());

        let mut solid = Solid3D::new();
        solid.set_sab_data(sab_data.clone());
        solid.common.layer = "0".to_string();

        let version = DxfVersion::AC1027; // R2013
        let mut doc = CadDocument::with_version(version);
        doc.add_entity(EntityType::Solid3D(solid))?;

        let path = "solid3d_r2013.dwg";
        DwgWriter::write_to_file(path, &doc)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!("[SAB] Written: {} ({} bytes, version: {:?})", path, size, version);
    }

    // ── 5. Read back and verify ─────────────────────────────────────
    {
        use acadrust::DwgReader;
        println!("\n=== Read-back verification ===");

        // Verify R2000 file
        let mut r0 = DwgReader::from_file("solid3d_r2000.dwg")?;
        let doc0 = r0.read()?;
        let solids0: Vec<&Solid3D> = doc0.entities().filter_map(|e| {
            if let EntityType::Solid3D(s) = e { Some(s) } else { None }
        }).collect();
        println!("[R2000] Entities read back: {} Solid3D", solids0.len());
        if let Some(s) = solids0.first() {
            println!("        has_data={}, is_binary={}, sat_len={}",
                s.acis_data.has_data(), s.acis_data.is_binary, s.acis_data.sat_data.len());
        }

        // Verify R2004 file
        let mut r1 = DwgReader::from_file("solid3d_r2004.dwg")?;
        let doc1 = r1.read()?;
        let solids1: Vec<&Solid3D> = doc1.entities().filter_map(|e| {
            if let EntityType::Solid3D(s) = e { Some(s) } else { None }
        }).collect();
        println!("[R2004] Entities read back: {} Solid3D", solids1.len());
        if let Some(s) = solids1.first() {
            println!("      has_data={}, is_binary={}, sat_len={}",
                s.acis_data.has_data(), s.acis_data.is_binary, s.acis_data.sat_data.len());
            if let Some(parsed) = s.parse_sat() {
                println!("      bodies={}, faces={}, edges={}, vertices={}",
                    parsed.bodies().len(), parsed.faces().len(),
                    parsed.edges().len(), parsed.vertices().len());
            }
        }

        // Verify SAB file
        let mut r2 = DwgReader::from_file("solid3d_r2013.dwg")?;
        let doc2 = r2.read()?;
        let solids2: Vec<&Solid3D> = doc2.entities().filter_map(|e| {
            if let EntityType::Solid3D(s) = e { Some(s) } else { None }
        }).collect();
        println!("[SAB] Entities read back: {} Solid3D", solids2.len());
        if let Some(s) = solids2.first() {
            println!("      has_data={}, is_binary={}, sab_len={}",
                s.acis_data.has_data(), s.acis_data.is_binary, s.acis_data.sab_data.len());
        }
    }

    println!("\nDone! Open solid3d_r2000.dwg, solid3d_r2004.dwg, or solid3d_r2013.dwg in AutoCAD/IntelliCAD.");
    Ok(())
}

/// Build a complete ACIS SAT document for a 10×10×10 axis-aligned box
/// centered at the origin (corners from (-5,-5,-5) to (5,5,5)).
fn build_box_sat() -> SatDocument {
    let mut sat = SatDocument::new_body();

    // The body record sits at index 0.
    let body_idx = SatPointer::new(0);

    // ════════════════════════════════════════════════════════════════
    //  Geometry (surfaces, curves, points)
    // ════════════════════════════════════════════════════════════════

    // 8 corner points
    //   p0(-5,-5,-5)  p1(5,-5,-5)  p2(5,5,-5)  p3(-5,5,-5)
    //   p4(-5,-5, 5)  p5(5,-5, 5)  p6(5,5, 5)  p7(-5,5, 5)
    let p0 = sat.add_point(-5.0, -5.0, -5.0);
    let p1 = sat.add_point( 5.0, -5.0, -5.0);
    let p2 = sat.add_point( 5.0,  5.0, -5.0);
    let p3 = sat.add_point(-5.0,  5.0, -5.0);
    let p4 = sat.add_point(-5.0, -5.0,  5.0);
    let p5 = sat.add_point( 5.0, -5.0,  5.0);
    let p6 = sat.add_point( 5.0,  5.0,  5.0);
    let p7 = sat.add_point(-5.0,  5.0,  5.0);

    // 6 plane surfaces  (origin, normal, u-vector)
    let surf_top    = sat.add_plane_surface([0.0, 0.0,  5.0], [0.0, 0.0,  1.0], [1.0, 0.0, 0.0]);
    let surf_bottom = sat.add_plane_surface([0.0, 0.0, -5.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]);
    let surf_front  = sat.add_plane_surface([0.0, -5.0, 0.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]);
    let surf_back   = sat.add_plane_surface([0.0,  5.0, 0.0], [0.0,  1.0, 0.0], [1.0, 0.0, 0.0]);
    let surf_right  = sat.add_plane_surface([ 5.0, 0.0, 0.0], [ 1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let surf_left   = sat.add_plane_surface([-5.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);

    // 12 straight curves  (start-point, direction)
    // Bottom face edges (Z = -5)
    let crv_b0 = sat.add_straight_curve([-5.0, -5.0, -5.0], [ 1.0, 0.0, 0.0]); // p0→p1
    let crv_b1 = sat.add_straight_curve([ 5.0, -5.0, -5.0], [ 0.0, 1.0, 0.0]); // p1→p2
    let crv_b2 = sat.add_straight_curve([ 5.0,  5.0, -5.0], [-1.0, 0.0, 0.0]); // p2→p3
    let crv_b3 = sat.add_straight_curve([-5.0,  5.0, -5.0], [ 0.0,-1.0, 0.0]); // p3→p0
    // Top face edges (Z = 5)
    let crv_t0 = sat.add_straight_curve([-5.0, -5.0,  5.0], [ 1.0, 0.0, 0.0]); // p4→p5
    let crv_t1 = sat.add_straight_curve([ 5.0, -5.0,  5.0], [ 0.0, 1.0, 0.0]); // p5→p6
    let crv_t2 = sat.add_straight_curve([ 5.0,  5.0,  5.0], [-1.0, 0.0, 0.0]); // p6→p7
    let crv_t3 = sat.add_straight_curve([-5.0,  5.0,  5.0], [ 0.0,-1.0, 0.0]); // p7→p4
    // Vertical edges
    let crv_v0 = sat.add_straight_curve([-5.0, -5.0, -5.0], [ 0.0, 0.0, 1.0]); // p0→p4
    let crv_v1 = sat.add_straight_curve([ 5.0, -5.0, -5.0], [ 0.0, 0.0, 1.0]); // p1→p5
    let crv_v2 = sat.add_straight_curve([ 5.0,  5.0, -5.0], [ 0.0, 0.0, 1.0]); // p2→p6
    let crv_v3 = sat.add_straight_curve([-5.0,  5.0, -5.0], [ 0.0, 0.0, 1.0]); // p3→p7

    // ════════════════════════════════════════════════════════════════
    //  Topology (vertices, edges, coedges, loops, faces, shell, lump)
    // ════════════════════════════════════════════════════════════════

    // 8 vertices
    let v0 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p0));
    let v1 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p1));
    let v2 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p2));
    let v3 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p3));
    let v4 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p4));
    let v5 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p5));
    let v6 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p6));
    let v7 = sat.add_vertex(SatPointer::NULL, SatPointer::new(p7));

    // 12 edges  (start_vertex, start_t, end_vertex, end_t, coedge=NULL, curve, sense)
    let e0  = sat.add_edge(SatPointer::new(v0), 0.0, SatPointer::new(v1), 10.0, SatPointer::NULL, SatPointer::new(crv_b0), Sense::Forward);
    let e1  = sat.add_edge(SatPointer::new(v1), 0.0, SatPointer::new(v2), 10.0, SatPointer::NULL, SatPointer::new(crv_b1), Sense::Forward);
    let e2  = sat.add_edge(SatPointer::new(v2), 0.0, SatPointer::new(v3), 10.0, SatPointer::NULL, SatPointer::new(crv_b2), Sense::Forward);
    let e3  = sat.add_edge(SatPointer::new(v3), 0.0, SatPointer::new(v0), 10.0, SatPointer::NULL, SatPointer::new(crv_b3), Sense::Forward);
    let e4  = sat.add_edge(SatPointer::new(v4), 0.0, SatPointer::new(v5), 10.0, SatPointer::NULL, SatPointer::new(crv_t0), Sense::Forward);
    let e5  = sat.add_edge(SatPointer::new(v5), 0.0, SatPointer::new(v6), 10.0, SatPointer::NULL, SatPointer::new(crv_t1), Sense::Forward);
    let e6  = sat.add_edge(SatPointer::new(v6), 0.0, SatPointer::new(v7), 10.0, SatPointer::NULL, SatPointer::new(crv_t2), Sense::Forward);
    let e7  = sat.add_edge(SatPointer::new(v7), 0.0, SatPointer::new(v4), 10.0, SatPointer::NULL, SatPointer::new(crv_t3), Sense::Forward);
    let e8  = sat.add_edge(SatPointer::new(v0), 0.0, SatPointer::new(v4), 10.0, SatPointer::NULL, SatPointer::new(crv_v0), Sense::Forward);
    let e9  = sat.add_edge(SatPointer::new(v1), 0.0, SatPointer::new(v5), 10.0, SatPointer::NULL, SatPointer::new(crv_v1), Sense::Forward);
    let e10 = sat.add_edge(SatPointer::new(v2), 0.0, SatPointer::new(v6), 10.0, SatPointer::NULL, SatPointer::new(crv_v2), Sense::Forward);
    let e11 = sat.add_edge(SatPointer::new(v3), 0.0, SatPointer::new(v7), 10.0, SatPointer::NULL, SatPointer::new(crv_v3), Sense::Forward);

    // ── Pre-compute indices for 24 coedges + 6 loops + 6 faces + shell + lump
    let base = sat.records.len() as i32;
    let co_base   = base;         // 24 coedges: base+0..23
    let loop_base = base + 24;    // 6 loops:   base+24..29
    let face_base = base + 30;    // 6 faces:   base+30..35
    let shell_idx = base + 36;
    let lump_idx  = base + 37;

    let ptr = |i: i32| SatPointer::new(i);

    // Coedge aliases for partner references
    let co = |i: i32| co_base + i;

    // ── Bottom face (Z = -5, normal outward = -Z) ───────────────────
    //    Loop: e0(rev) → e3(rev) → e2(rev) → e1(rev)
    sat.add_coedge(ptr(co(1)),  ptr(co(3)),  ptr(co(8)),  ptr(e0), Sense::Reversed, ptr(loop_base));     // co0, partner=front co8
    sat.add_coedge(ptr(co(2)),  ptr(co(0)),  ptr(co(20)), ptr(e3), Sense::Reversed, ptr(loop_base));     // co1, partner=left co20
    sat.add_coedge(ptr(co(3)),  ptr(co(1)),  ptr(co(12)), ptr(e2), Sense::Reversed, ptr(loop_base));     // co2, partner=back co12
    sat.add_coedge(ptr(co(0)),  ptr(co(2)),  ptr(co(16)), ptr(e1), Sense::Reversed, ptr(loop_base));     // co3, partner=right co16

    // ── Top face (Z = +5, normal outward = +Z) ─────────────────────
    //    Loop: e4(fwd) → e5(fwd) → e6(fwd) → e7(fwd)
    sat.add_coedge(ptr(co(5)),  ptr(co(7)),  ptr(co(10)), ptr(e4), Sense::Forward, ptr(loop_base + 1));  // co4, partner=front co10
    sat.add_coedge(ptr(co(6)),  ptr(co(4)),  ptr(co(18)), ptr(e5), Sense::Forward, ptr(loop_base + 1));  // co5, partner=right co18
    sat.add_coedge(ptr(co(7)),  ptr(co(5)),  ptr(co(14)), ptr(e6), Sense::Forward, ptr(loop_base + 1));  // co6, partner=back co14
    sat.add_coedge(ptr(co(4)),  ptr(co(6)),  ptr(co(22)), ptr(e7), Sense::Forward, ptr(loop_base + 1));  // co7, partner=left co22

    // ── Front face (Y = -5, normal outward = -Y) ───────────────────
    //    Loop: e0(fwd) → e9(fwd) → e4(rev) → e8(rev)
    sat.add_coedge(ptr(co(9)),  ptr(co(11)), ptr(co(0)),  ptr(e0), Sense::Forward,  ptr(loop_base + 2)); // co8, partner=bottom co0
    sat.add_coedge(ptr(co(10)), ptr(co(8)),  ptr(co(19)), ptr(e9), Sense::Forward,  ptr(loop_base + 2)); // co9, partner=right co19
    sat.add_coedge(ptr(co(11)), ptr(co(9)),  ptr(co(4)),  ptr(e4), Sense::Reversed, ptr(loop_base + 2)); // co10, partner=top co4
    sat.add_coedge(ptr(co(8)),  ptr(co(10)), ptr(co(21)), ptr(e8), Sense::Reversed, ptr(loop_base + 2)); // co11, partner=left co21

    // ── Back face (Y = +5, normal outward = +Y) ────────────────────
    //    Loop: e2(fwd) → e11(fwd) → e6(rev) → e10(rev)
    sat.add_coedge(ptr(co(13)), ptr(co(15)), ptr(co(2)),  ptr(e2),  Sense::Forward,  ptr(loop_base + 3)); // co12, partner=bottom co2
    sat.add_coedge(ptr(co(14)), ptr(co(12)), ptr(co(23)), ptr(e11), Sense::Forward,  ptr(loop_base + 3)); // co13, partner=left co23
    sat.add_coedge(ptr(co(15)), ptr(co(13)), ptr(co(6)),  ptr(e6),  Sense::Reversed, ptr(loop_base + 3)); // co14, partner=top co6
    sat.add_coedge(ptr(co(12)), ptr(co(14)), ptr(co(17)), ptr(e10), Sense::Reversed, ptr(loop_base + 3)); // co15, partner=right co17

    // ── Right face (X = +5, normal outward = +X) ───────────────────
    //    Loop: e1(fwd) → e10(fwd) → e5(rev) → e9(rev)
    sat.add_coedge(ptr(co(17)), ptr(co(19)), ptr(co(3)),  ptr(e1),  Sense::Forward,  ptr(loop_base + 4)); // co16, partner=bottom co3
    sat.add_coedge(ptr(co(18)), ptr(co(16)), ptr(co(15)), ptr(e10), Sense::Forward,  ptr(loop_base + 4)); // co17, partner=back co15
    sat.add_coedge(ptr(co(19)), ptr(co(17)), ptr(co(5)),  ptr(e5),  Sense::Reversed, ptr(loop_base + 4)); // co18, partner=top co5
    sat.add_coedge(ptr(co(16)), ptr(co(18)), ptr(co(9)),  ptr(e9),  Sense::Reversed, ptr(loop_base + 4)); // co19, partner=front co9

    // ── Left face (X = -5, normal outward = -X) ────────────────────
    //    Loop: e3(fwd) → e8(fwd) → e7(rev) → e11(rev)
    sat.add_coedge(ptr(co(21)), ptr(co(23)), ptr(co(1)),  ptr(e3),  Sense::Forward,  ptr(loop_base + 5)); // co20, partner=bottom co1
    sat.add_coedge(ptr(co(22)), ptr(co(20)), ptr(co(11)), ptr(e8),  Sense::Forward,  ptr(loop_base + 5)); // co21, partner=front co11
    sat.add_coedge(ptr(co(23)), ptr(co(21)), ptr(co(7)),  ptr(e7),  Sense::Reversed, ptr(loop_base + 5)); // co22, partner=top co7
    sat.add_coedge(ptr(co(20)), ptr(co(22)), ptr(co(13)), ptr(e11), Sense::Reversed, ptr(loop_base + 5)); // co23, partner=back co13

    // ── 6 Loops ─────────────────────────────────────────────────────
    sat.add_loop(SatPointer::NULL, ptr(co(0)),  ptr(face_base));
    sat.add_loop(SatPointer::NULL, ptr(co(4)),  ptr(face_base + 1));
    sat.add_loop(SatPointer::NULL, ptr(co(8)),  ptr(face_base + 2));
    sat.add_loop(SatPointer::NULL, ptr(co(12)), ptr(face_base + 3));
    sat.add_loop(SatPointer::NULL, ptr(co(16)), ptr(face_base + 4));
    sat.add_loop(SatPointer::NULL, ptr(co(20)), ptr(face_base + 5));

    // ── 6 Faces (linked list via next_face) ─────────────────────────
    sat.add_face(ptr(face_base + 1), ptr(loop_base),     ptr(shell_idx), ptr(surf_bottom), Sense::Forward, Sidedness::Single);
    sat.add_face(ptr(face_base + 2), ptr(loop_base + 1), ptr(shell_idx), ptr(surf_top),    Sense::Forward, Sidedness::Single);
    sat.add_face(ptr(face_base + 3), ptr(loop_base + 2), ptr(shell_idx), ptr(surf_front),  Sense::Forward, Sidedness::Single);
    sat.add_face(ptr(face_base + 4), ptr(loop_base + 3), ptr(shell_idx), ptr(surf_back),   Sense::Forward, Sidedness::Single);
    sat.add_face(ptr(face_base + 5), ptr(loop_base + 4), ptr(shell_idx), ptr(surf_right),  Sense::Forward, Sidedness::Single);
    sat.add_face(SatPointer::NULL,   ptr(loop_base + 5), ptr(shell_idx), ptr(surf_left),   Sense::Forward, Sidedness::Single);

    // ── Shell → Lump → Body ─────────────────────────────────────────
    sat.add_shell(ptr(face_base), ptr(lump_idx));
    sat.add_lump(ptr(shell_idx), body_idx);

    // Patch the body record to point to the lump
    if let Some(body_rec) = sat.record_mut(0) {
        body_rec.tokens[1] = SatToken::Pointer(ptr(lump_idx));
    }

    sat
}
