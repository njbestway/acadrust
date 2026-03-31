use acadrust::DwgReader;
use acadrust::entities::EntityType;
use std::collections::HashSet;

fn main() {
    let path = "tests/issue14/General.dwg";
    let mut reader = DwgReader::from_file(path).expect("Failed to open");
    let doc = reader.read().expect("Failed to read");

    // Failing handles from BricsCAD
    let failing: HashSet<u64> = [
        0x2413Du64, 0x2413F, 0x24156, 0x24186, 0x24189, 0x2418C, 0x2418F,
        0x24192, 0x24195, 0x24198, 0x2419B, 0x2419E, 0x241A1, 0x241A4,
        0x241A7, 0x241AE, 0x241B6, 0x241B9, 0x241BF, 0x241CE, 0x241E6,
        0x2420D, 0x24211, 0x24217, 0x2421C, 0x2421F, 0x24223, 0x24226,
        0x2422E, 0x24237, 0x2423A, 0x2423C, 0x2423E, 0x24242, 0x24248,
        0x2424B, 0x2424E, 0x24251, 0x2425D, 0x2425F, 0x24266, 0x24270,
        0x2427C, 0x2427F, 0x24285, 0x24288, 0x24295, 0x24298, 0x2429B,
        0x2429E, 0x242A1, 0x242AC, 0x242AF, 0x242B1, 0x242B6, 0x242BC,
        0x242EA, 0x242F1, 0x24302, 0x24303, 0x24304, 0x24305, 0x2430E,
        0x2430F, 0x24312, 0x24315, 0x24318, 0x2431B, 0x2431E, 0x24321,
        0x24324, 0x24327, 0x2432A, 0x2432D, 0x24330, 0x24333, 0x24336,
        0x24339, 0x2433C, 0x2433F, 0x24342, 0x24345,
    ].iter().copied().collect();

    println!("=== LwPolyline Property Comparison ===");
    println!("Failing count: {}", failing.len());

    // Track property distributions
    let mut fail_has_xdata = 0;
    let mut pass_has_xdata = 0;
    let mut fail_has_reactors = 0;
    let mut pass_has_reactors = 0;
    let mut fail_has_xdic = 0;
    let mut pass_has_xdic = 0;
    let mut fail_has_transparency = 0;
    let mut pass_has_transparency = 0;
    let mut fail_is_rgb = 0;
    let mut pass_is_rgb = 0;
    let mut fail_has_bulges = 0;
    let mut pass_has_bulges = 0;
    let mut fail_has_widths = 0;
    let mut pass_has_widths = 0;
    let mut fail_has_thickness = 0;
    let mut pass_has_thickness = 0;
    let mut fail_has_elevation = 0;
    let mut pass_has_elevation = 0;
    let mut fail_has_normal = 0;
    let mut pass_has_normal = 0;
    let mut fail_has_constw = 0;
    let mut pass_has_constw = 0;
    let mut fail_closed = 0;
    let mut pass_closed = 0;
    let mut fail_invisible = 0;
    let mut pass_invisible = 0;
    let mut fail_named_lt = 0;
    let mut pass_named_lt = 0;
    let mut fail_total = 0;
    let mut pass_total = 0;

    for entity in doc.entities() {
        if let EntityType::LwPolyline(lw) = entity {
            let h: u64 = lw.common.handle.value();
            let is_fail = failing.contains(&h);
            let c = &lw.common;

            let has_xdata = !c.extended_data.is_empty();
            let has_reactors = !c.reactors.is_empty();
            let has_xdic = c.xdictionary_handle.is_some();
            let has_transparency = !c.transparency.is_opaque();
            let is_rgb = matches!(c.color, acadrust::types::Color::Rgb { .. });
            let has_bulges = lw.vertices.iter().any(|v| v.bulge != 0.0);
            let has_widths = lw.vertices.iter().any(|v| v.start_width != 0.0 || v.end_width != 0.0);
            let has_thickness = lw.thickness != 0.0;
            let has_elevation = lw.elevation != 0.0;
            let has_normal = lw.normal != acadrust::types::Vector3::UNIT_Z;
            let has_constw = lw.constant_width != 0.0;
            let lt_lower = c.linetype.to_ascii_lowercase();
            let named_lt = !lt_lower.is_empty() && lt_lower != "bylayer" && lt_lower != "byblock" && lt_lower != "continuous";

            if is_fail {
                fail_total += 1;
                if has_xdata { fail_has_xdata += 1; }
                if has_reactors { fail_has_reactors += 1; }
                if has_xdic { fail_has_xdic += 1; }
                if has_transparency { fail_has_transparency += 1; }
                if is_rgb { fail_is_rgb += 1; }
                if has_bulges { fail_has_bulges += 1; }
                if has_widths { fail_has_widths += 1; }
                if has_thickness { fail_has_thickness += 1; }
                if has_elevation { fail_has_elevation += 1; }
                if has_normal { fail_has_normal += 1; }
                if has_constw { fail_has_constw += 1; }
                if lw.is_closed { fail_closed += 1; }
                if c.invisible { fail_invisible += 1; }
                if named_lt { fail_named_lt += 1; }
            } else {
                pass_total += 1;
                if has_xdata { pass_has_xdata += 1; }
                if has_reactors { pass_has_reactors += 1; }
                if has_xdic { pass_has_xdic += 1; }
                if has_transparency { pass_has_transparency += 1; }
                if is_rgb { pass_is_rgb += 1; }
                if has_bulges { pass_has_bulges += 1; }
                if has_widths { pass_has_widths += 1; }
                if has_thickness { pass_has_thickness += 1; }
                if has_elevation { pass_has_elevation += 1; }
                if has_normal { pass_has_normal += 1; }
                if has_constw { pass_has_constw += 1; }
                if lw.is_closed { pass_closed += 1; }
                if c.invisible { pass_invisible += 1; }
                if named_lt { pass_named_lt += 1; }
            }
        }
    }

    println!("\nProperty         FAIL({})   PASS({})", fail_total, pass_total);
    println!("has_xdata        {:>5}      {:>5}", fail_has_xdata, pass_has_xdata);
    println!("has_reactors     {:>5}      {:>5}", fail_has_reactors, pass_has_reactors);
    println!("has_xdic         {:>5}      {:>5}", fail_has_xdic, pass_has_xdic);
    println!("has_transparency {:>5}      {:>5}", fail_has_transparency, pass_has_transparency);
    println!("is_rgb_color     {:>5}      {:>5}", fail_is_rgb, pass_is_rgb);
    println!("has_bulges       {:>5}      {:>5}", fail_has_bulges, pass_has_bulges);
    println!("has_widths       {:>5}      {:>5}", fail_has_widths, pass_has_widths);
    println!("has_thickness    {:>5}      {:>5}", fail_has_thickness, pass_has_thickness);
    println!("has_elevation    {:>5}      {:>5}", fail_has_elevation, pass_has_elevation);
    println!("has_normal       {:>5}      {:>5}", fail_has_normal, pass_has_normal);
    println!("has_constwidth   {:>5}      {:>5}", fail_has_constw, pass_has_constw);
    println!("is_closed        {:>5}      {:>5}", fail_closed, pass_closed);
    println!("invisible        {:>5}      {:>5}", fail_invisible, pass_invisible);
    println!("named_linetype   {:>5}      {:>5}", fail_named_lt, pass_named_lt);

    // Print first few failing entities in detail
    println!("\n=== FIRST 5 FAILING LwPolylines ===");
    let mut count = 0;
    let mut sorted_fails: Vec<u64> = failing.iter().copied().collect();
    sorted_fails.sort();
    for h in &sorted_fails {
        if count >= 5 { break; }
        let handle = acadrust::types::Handle::from(*h);
        if let Some(EntityType::LwPolyline(lw)) = doc.get_entity(handle) {
            let c = &lw.common;
            println!("\nHandle {:#X}:", h);
            println!("  layer='{}' color={:?} linetype='{}' lw={:?} ltscale={}",
                c.layer, c.color, c.linetype, c.line_weight, c.linetype_scale);
            println!("  owner={:?} invisible={} transparency={:?}",
                c.owner_handle, c.invisible, c.transparency);
            println!("  xdata={} reactors={:?} xdic={:?}",
                !c.extended_data.is_empty(), c.reactors, c.xdictionary_handle);
            println!("  vertices={} closed={} elevation={} thickness={}",
                lw.vertices.len(), lw.is_closed, lw.elevation, lw.thickness);
            println!("  normal={:?} const_width={}", lw.normal, lw.constant_width);
            if lw.vertices.len() > 0 {
                println!("  first_vertex: ({}, {}) bulge={} sw={} ew={}",
                    lw.vertices[0].location.x, lw.vertices[0].location.y,
                    lw.vertices[0].bulge, lw.vertices[0].start_width, lw.vertices[0].end_width);
            }
            count += 1;
        }
    }

    // Print first few PASSING in same handle range for comparison
    println!("\n=== FIRST 5 PASSING LwPolylines (handle > 0x24100) ===");
    count = 0;
    for entity in doc.entities() {
        if count >= 5 { break; }
        let hv: u64 = entity.common().handle.value();
        if hv < 0x24100 { continue; }
        if failing.contains(&hv) { continue; }
        if let EntityType::LwPolyline(lw) = entity {
            let c = &lw.common;
            println!("\nHandle {:#X}:", hv);
            println!("  layer='{}' color={:?} linetype='{}' lw={:?} ltscale={}",
                c.layer, c.color, c.linetype, c.line_weight, c.linetype_scale);
            println!("  owner={:?} invisible={} transparency={:?}",
                c.owner_handle, c.invisible, c.transparency);
            println!("  xdata={} reactors={:?} xdic={:?}",
                !c.extended_data.is_empty(), c.reactors, c.xdictionary_handle);
            println!("  vertices={} closed={} elevation={} thickness={}",
                lw.vertices.len(), lw.is_closed, lw.elevation, lw.thickness);
            println!("  normal={:?} const_width={}", lw.normal, lw.constant_width);
            if lw.vertices.len() > 0 {
                println!("  first_vertex: ({}, {}) bulge={} sw={} ew={}",
                    lw.vertices[0].location.x, lw.vertices[0].location.y,
                    lw.vertices[0].bulge, lw.vertices[0].start_width, lw.vertices[0].end_width);
            }
            count += 1;
        }
    }
}
