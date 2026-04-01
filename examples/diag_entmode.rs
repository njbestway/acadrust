/// Diagnostic: verify entity_mode and owner handles for multi-paper-space documents.
///
/// Creates a document with:
///   - *Model_Space with a line
///   - *Paper_Space (canonical) with a circle
///   - A second paper space (*Paper_Space0) with an arc
///
/// Then roundtrips through DWG and checks entity owner preservation.
use std::io::Cursor;

use acadrust::entities::*;
use acadrust::tables::TableEntry;
use acadrust::types::{DxfVersion, Vector3};
use acadrust::{CadDocument, DwgReader, DwgWriter};

fn main() {
    // ── 1. Build document with multiple paper spaces ──
    let mut doc = CadDocument::with_version(DxfVersion::AC1032);

    // Add a line to model space
    let mut line = Line::new();
    line.start = Vector3::new(0.0, 0.0, 0.0);
    line.end = Vector3::new(10.0, 10.0, 0.0);
    let line_h = doc.add_entity(EntityType::Line(line)).unwrap();
    println!("[BUILD] Line {:#X} → *Model_Space", line_h.value());

    // Add a circle to canonical *Paper_Space
    let mut circle = Circle::new();
    circle.center = Vector3::new(5.0, 5.0, 0.0);
    circle.radius = 3.0;
    let circle_h = doc.add_paper_space_entity(EntityType::Circle(circle)).unwrap();
    println!("[BUILD] Circle {:#X} → *Paper_Space", circle_h.value());

    // Create a second paper space block record
    let ps2_name = "*Paper_Space0";
    let ps2_handle = doc.allocate_handle();
    {
        let mut br = acadrust::tables::BlockRecord::new(ps2_name);
        br.handle = ps2_handle;
        doc.block_records.add_or_replace(br);
    }
    println!("[BUILD] Block '{}' handle={:#X}", ps2_name, ps2_handle.value());

    // Add an arc to the second paper space via add_entity_to_layout won't work,
    // so we do it manually: add to model space then move
    let mut arc = Arc::new();
    arc.center = Vector3::new(1.0, 1.0, 0.0);
    arc.radius = 2.0;
    arc.start_angle = 0.0;
    arc.end_angle = std::f64::consts::PI;
    // Add to model space first, then redirect
    let arc_h = doc.add_entity(EntityType::Arc(arc)).unwrap();

    // Move the arc from *Model_Space to *Paper_Space0
    // 1. Remove from Model_Space entity_handles
    if let Some(ms) = doc.block_records.get_mut("*Model_Space") {
        ms.entity_handles.retain(|h| *h != arc_h);
    }
    // 2. Add to Paper_Space0 entity_handles
    if let Some(ps2) = doc.block_records.get_mut(ps2_name) {
        ps2.entity_handles.push(arc_h);
    }
    // 3. Update entity's owner_handle
    if let Some(e) = doc.get_entity_mut(arc_h) {
        e.common_mut().owner_handle = ps2_handle;
    }
    println!("[BUILD] Arc {:#X} → {} ({:#X})", arc_h.value(), ps2_name, ps2_handle.value());

    // Print all block records and their entities
    println!("\n=== Block Records (before write) ===");
    for br in doc.block_records.iter() {
        println!("  {} (handle={:#X})", br.name(), br.handle.value());
        for eh in &br.entity_handles {
            if let Some(e) = doc.get_entity(*eh) {
                println!("    entity {:#X} type={} owner={:#X}",
                    eh.value(),
                    entity_type_name(e),
                    e.common().owner_handle.value()
                );
            }
        }
    }

    // Verify entity_mode computation
    let ms_handle = doc.block_records.get("*Model_Space").map(|br| br.handle);
    let ps_handle = doc.block_records.get("*Paper_Space").map(|br| br.handle);
    println!("\n=== Entity Mode Check ===");
    println!("  *Model_Space handle: {:?}", ms_handle);
    println!("  *Paper_Space handle: {:?}", ps_handle);
    println!("  *Paper_Space0 handle: {:?}", Some(ps2_handle));
    for e in doc.entities() {
        let owner = e.common().owner_handle;
        let entmode = if ms_handle.map_or(false, |ms| owner == ms) {
            2
        } else if ps_handle.map_or(false, |ps| owner == ps) {
            1
        } else {
            0
        };
        println!("  entity {:#X} ({}) owner={:#X} → entmode={}{}",
            e.common().handle.value(),
            entity_type_name(e),
            owner.value(),
            entmode,
            if entmode == 0 { " (owner handle WILL be written)" } else { " (owner handle OMITTED)" },
        );
    }

    // ── 2. Write to DWG buffer ──
    println!("\n=== Writing DWG ===");
    let mut buf = Cursor::new(Vec::new());
    DwgWriter::write_to_writer(&mut buf, &doc).expect("DWG write failed");
    let bytes = buf.into_inner();
    println!("Wrote {} bytes", bytes.len());

    // ── 3. Read back ──
    println!("\n=== Reading back DWG ===");
    let cursor = Cursor::new(&bytes);
    let mut reader = DwgReader::from_stream(cursor);
    let doc2 = reader.read().expect("DWG read failed");

    // Check notifications for any owner corrections
    println!("\n=== Reader Notifications ===");
    for n in reader.notifications.iter() {
        println!("  {:?}", n);
    }

    // Print all block records and their entities in read-back
    println!("\n=== Block Records (after read-back) ===");
    for br in doc2.block_records.iter() {
        println!("  {} (handle={:#X})", br.name(), br.handle.value());
        for eh in &br.entity_handles {
            if let Some(e) = doc2.get_entity(*eh) {
                println!("    entity {:#X} type={} owner={:#X}",
                    eh.value(),
                    entity_type_name(e),
                    e.common().owner_handle.value()
                );
            }
        }
    }

    // ── 4. Compare ──
    println!("\n=== Owner Comparison ===");
    let mut any_mismatch = false;
    for e1 in doc.entities() {
        let h = e1.common().handle;
        let owner1 = e1.common().owner_handle;
        if let Some(e2) = doc2.get_entity(h) {
            let owner2 = e2.common().owner_handle;
            let status = if owner1 == owner2 { "OK" } else { "MISMATCH" };
            if owner1 != owner2 { any_mismatch = true; }
            println!("  {:#X} ({}) wrote_owner={:#X} read_owner={:#X} [{}]",
                h.value(), entity_type_name(e1), owner1.value(), owner2.value(), status);
        } else {
            any_mismatch = true;
            println!("  {:#X} ({}) wrote_owner={:#X} NOT FOUND!", h.value(), entity_type_name(e1), owner1.value());
        }
    }
    if any_mismatch {
        println!("\n*** OWNER MISMATCH DETECTED ***");
    } else {
        println!("\nAll owners match.");
    }

    // ── 5. File-level roundtrip (write doc2 and read back again) ──
    // This simulates what happens with a real DWG file that was originally
    // read from disk (where entities had entity_mode=1 and were corrected).
    println!("\n=== Double roundtrip ===");
    let mut buf2 = Cursor::new(Vec::new());
    DwgWriter::write_to_writer(&mut buf2, &doc2).expect("DWG write2 failed");
    let bytes2 = buf2.into_inner();
    let cursor2 = Cursor::new(&bytes2);
    let mut reader2 = DwgReader::from_stream(cursor2);
    let doc3 = reader2.read().expect("DWG read2 failed");

    println!("\n=== Double Roundtrip Owner Comparison ===");
    let mut any_mismatch2 = false;
    for e2 in doc2.entities() {
        let h = e2.common().handle;
        let owner2 = e2.common().owner_handle;
        if let Some(e3) = doc3.get_entity(h) {
            let owner3 = e3.common().owner_handle;
            let status = if owner2 == owner3 { "OK" } else { "MISMATCH" };
            if owner2 != owner3 { any_mismatch2 = true; }
            println!("  {:#X} ({}) rt1_owner={:#X} rt2_owner={:#X} [{}]",
                h.value(), entity_type_name(e2), owner2.value(), owner3.value(), status);
        } else {
            any_mismatch2 = true;
            println!("  {:#X} ({}) rt1_owner={:#X} NOT FOUND!", h.value(), entity_type_name(e2), owner2.value());
        }
    }
    if any_mismatch2 {
        println!("\n*** DOUBLE ROUNDTRIP MISMATCH ***");
    } else {
        println!("\nDouble roundtrip: all owners match.");
    }
}

fn entity_type_name(e: &EntityType) -> &'static str {
    match e {
        EntityType::Line(_) => "LINE",
        EntityType::Circle(_) => "CIRCLE",
        EntityType::Arc(_) => "ARC",
        EntityType::Point(_) => "POINT",
        EntityType::Block(_) => "BLOCK",
        EntityType::BlockEnd(_) => "ENDBLK",
        _ => "OTHER",
    }
}
