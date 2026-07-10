//! dxf2json — DXF/DWG to GeoJSON 转换工具
//!
//! 读取 DXF 或 DWG 文件，按图层输出 GeoJSON FeatureCollection。
//! 支持多文件输入、输出目录配置、图层过滤等 CLI 参数。
//!
//! # 用法
//! ```sh
//! dxf2json -i input.dwg -o output/
//! dxf2json -i a.dxf b.dwg -o result/ --layers WALL AXIS
//! ```

use acadrust::entities::*;
use acadrust::objects::ObjectType;
use acadrust::types::{Color, Matrix3, Vector2, Vector3};
use acadrust::{CadDocument, DwgReader, DxfReader, EntityType};
use clap::Parser;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

// ── CLI 参数定义 ──────────────────────────────────────────────

/// DXF/DWG to GeoJSON 转换器
#[derive(Parser, Debug)]
#[command(name = "dxf2json", version, about)]
struct Cli {
    /// 输入文件路径（支持 .dxf 和 .dwg，可指定多个）
    #[arg(short = 'i', long = "input", required = true, num_args = 1..)]
    input: Vec<String>,

    /// 输出目录（每个图层生成一个 JSON 文件）
    #[arg(short = 'o', long = "output", default_value = "output")]
    output: String,

    /// 只转换指定图层（不指定则转换所有图层）
    #[arg(short = 'l', long = "layers", num_args = 1..)]
    layers: Vec<String>,

    /// 安静模式：仅输出错误信息
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,

    /// 合并 XREF 绑定图层（将 "图层名 @ N" 合并到基础图层）
    #[arg(long = "merge-xref")]
    merge_xref: bool,
}

// ── 弧线离散化最小角度步长（弧度），6° ──
const SMALLEST_ANGLE: f64 = 6.0 * std::f64::consts::PI / 180.0;

fn main() -> acadrust::Result<()> {
    let cli = Cli::parse();

    // 创建输出目录
    fs::create_dir_all(&cli.output).map_err(|e| {
        acadrust::error::DxfError::Io(e)
    })?;

    for input_file in &cli.input {
        if !Path::new(input_file).exists() {
            eprintln!("[ERROR] File not found: {}", input_file);
            continue;
        }
        if let Err(e) = process_file(input_file, &cli) {
            eprintln!("[ERROR] Failed to process {}: {}", input_file, e);
        }
    }

    Ok(())
}

/// 处理单个 DXF/DWG 文件
fn process_file(input_file: &str, cli: &Cli) -> acadrust::Result<()> {
    // 1. 读取文件
    let ext = Path::new(input_file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("dxf")
        .to_lowercase();

    let doc = if ext == "dwg" {
        if !cli.quiet { println!("Reading DWG: {}", input_file); }
        let mut reader = DwgReader::from_file(input_file)?;
        reader.read()?
    } else {
        if !cli.quiet { println!("Reading DXF: {}", input_file); }
        DxfReader::from_file(input_file)?.read()?
    };

    if !cli.quiet {
        println!(
            "  Version: {}, Layers: {}, Entities: {}",
            doc.version.as_str(),
            doc.layers.iter().count(),
            doc.entities().count()
        );
    }

    // 2. 展开所有 Insert（块引用）
    let mut exploded_by_layer: std::collections::HashMap<String, Vec<EntityType>> =
        std::collections::HashMap::new();

    for entity in doc.entities() {
        if let EntityType::Insert(ins) = entity {
            let exploded = ins.explode_from_document(&doc);
            for sub_entity in exploded {
                let layer = sub_entity.common().layer.clone();
                exploded_by_layer
                    .entry(layer)
                    .or_default()
                    .push(sub_entity);
            }
            for attrib in &ins.attributes {
                let layer = attrib.common.layer.clone();
                exploded_by_layer
                    .entry(layer)
                    .or_default()
                    .push(EntityType::AttributeEntity(attrib.clone()));
            }
        }
    }

    let exploded_by_layer = exploded_by_layer;

    // 3. 确定要处理的图层
    let all_layer_names: Vec<String> = if cli.layers.is_empty() {
        doc.layers.iter().map(|l| l.name.clone()).collect()
    } else {
        cli.layers.clone()
    };

    // 4. 按图层输出
    let mut layer_count = 0u32;

    // 当 --merge-xref 启用时，将 "图层名 @ N" 分组到基础图层名
    let layer_groups: Vec<(String, Vec<String>)> = if cli.merge_xref {
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for name in &all_layer_names {
            let base = xref_base_name(name).to_string();
            if !groups.contains_key(&base) {
                order.push(base.clone());
            }
            groups.entry(base).or_default().push(name.clone());
        }
        order.into_iter().map(|b| {
            let variants = groups.remove(&b).unwrap_or_default();
            (b, variants)
        }).collect()
    } else {
        all_layer_names.into_iter().map(|n| (n.clone(), vec![n])).collect()
    };

    for (output_name, variants) in &layer_groups {
        let mut features = Vec::new();
        for variant in variants {
            features.extend(collect_layer_features(&doc, variant, &exploded_by_layer));
        }
        if features.is_empty() {
            continue;
        }

        // 以基础图层的属性为主，不存在时 fallback 到第一个变体
        let layer = doc.layers.get(output_name.as_str())
            .or_else(|| variants.iter().find_map(|v| doc.layers.get(v.as_str())));
        let visible = layer.map(|l| !l.flags.off).unwrap_or(true);
        let is_base_layer = output_name == "定位基准线";

        let mut fc = Map::new();
        fc.insert("type".into(), json!("FeatureCollection"));
        fc.insert("layerName".into(), json!(output_name));
        fc.insert("visible".into(), json!(if visible { "1" } else { "0" }));
        fc.insert("layerType".into(), json!(if is_base_layer { "1" } else { "0" }));

        if let Some(l) = layer {
            fc.insert("layerCode".into(), json!(format!("L{}", l.handle.value())));
        }

        let crs = read_crs_from_doc(&doc, cli.quiet);
        fc.insert("crs".into(), crs);

        let filtered: Vec<Value> = features.into_iter().filter(|f| !has_empty_coords(f)).collect();
        fc.insert("features".into(), Value::Array(filtered));

        let safe_name = output_name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        let output_file = format!("{}/{}.json", cli.output, safe_name);
        fs::write(
            &output_file,
            serde_json::to_string_pretty(&Value::Object(fc))
                .map_err(|e| acadrust::error::DxfError::Custom(e.to_string()))?,
        )?;
        layer_count += 1;
        if !cli.quiet { println!("  Written: {}", output_file); }
    }

    if !cli.quiet {
        println!("  Output: {} layer files in {}/", layer_count, cli.output);
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════
//  图层 Feature 收集
// ═══════════════════════════════════════════════════════════════

fn collect_layer_features(
    doc: &CadDocument,
    layer_name: &str,
    exploded_by_layer: &std::collections::HashMap<String, Vec<EntityType>>,
) -> Vec<Value> {
    if layer_name == "定位基准线" {
        return collect_positioning_baseline_features(doc, layer_name);
    }

    let mut features = Vec::new();

    for entity in doc.entities().filter(|e| e.common().layer == layer_name) {
        if matches!(entity, EntityType::Insert(_) | EntityType::AttributeDefinition(_)) {
            continue;
        }
        if let Some(fs) = entity_to_features(entity) {
            features.extend(fs);
        }
    }

    if let Some(exploded) = exploded_by_layer.get(layer_name) {
        for entity in exploded {
            if let Some(fs) = entity_to_features(entity) {
                features.extend(fs);
            }
        }
    }

    features
}

fn entity_to_features(entity: &EntityType) -> Option<Vec<Value>> {
    let features = match entity {
        EntityType::Line(e) => vec![line_to_feature(e)],
        EntityType::Point(e) => vec![point_to_feature(e)],
        EntityType::Circle(e) => vec![circle_to_feature(e)],
        EntityType::Arc(e) => vec![arc_to_feature(e)],
        EntityType::Text(e) => vec![text_to_feature(e)],
        EntityType::MText(e) => vec![mtext_to_feature(e)],
        EntityType::AttributeEntity(e) => vec![attrib_to_feature(e)],
        EntityType::LwPolyline(e) => lwpolyline_to_features(e),
        EntityType::Polyline(e) => vec![polyline3d_to_feature(e)],
        EntityType::Polyline2D(e) => vec![polyline2d_to_feature(e)],
        EntityType::Ellipse(e) => vec![ellipse_to_feature(e)],
        EntityType::Spline(e) => vec![spline_to_feature(e)],
        EntityType::Hatch(e) => vec![hatch_to_feature(e)],
        EntityType::Solid(e) => vec![solid_to_feature(e)],
        EntityType::Face3D(e) => vec![face3d_to_feature(e)],
        EntityType::Dimension(e) => dimension_to_features(e),
        EntityType::Helix(e) => vec![helix_to_feature(e)],
        _ => return None,
    };
    Some(features)
}

// ═══════════════════════════════════════════════════════════════
//  XREF 图层名处理
// ═══════════════════════════════════════════════════════════════

/// 剥离 AutoCAD XREF 绑定产生的 " @ N" 后缀
/// "坐标网格 @ 1" -> "坐标网格"，"坐标网格" -> "坐标网格"
fn xref_base_name(layer_name: &str) -> &str {
    if let Some(pos) = layer_name.rfind(" @ ") {
        let suffix = &layer_name[pos + 3..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            return &layer_name[..pos];
        }
    }
    layer_name
}

// ═══════════════════════════════════════════════════════════════
//  坐标转换辅助函数
// ═══════════════════════════════════════════════════════════════

fn ocs_to_wcs(normal: Vector3, point: Vector3) -> Vector3 {
    let basis = Matrix3::arbitrary_axis(normal);
    basis * point
}

fn pt(v: Vector3) -> Value {
    json!([v.x, v.y, v.z])
}

fn color_to_rgb_string(color: Color) -> String {
    let (r, g, b) = match color {
        Color::Rgb { r, g, b } => (r, g, b),
        Color::Index(i) => Color::Index(i).rgb().unwrap_or((255, 255, 255)),
        Color::ByLayer => (255, 255, 255),
        Color::ByBlock => (0, 0, 0),
    };
    format!("{},{},{}", r, g, b)
}

fn make_feature(geom_type: &str, coordinates: Value, properties: Value) -> Value {
    json!({
        "type": "Feature",
        "geometry": { "type": geom_type, "coordinates": coordinates },
        "properties": properties
    })
}

fn make_feature_with_code(geom_type: &str, coordinates: Value, properties: Value, code: u64) -> Value {
    json!({
        "type": "Feature",
        "code": code,
        "geometry": { "type": geom_type, "coordinates": coordinates },
        "properties": properties
    })
}

// ═══════════════════════════════════════════════════════════════
//  弧线离散化
// ═══════════════════════════════════════════════════════════════

#[allow(dead_code)]
fn tessellate_arc(
    center: Vector3, radius: f64, start_angle: f64, end_angle: f64, normal: Vector3,
) -> Vec<Vector3> {
    let mut pts = Vec::new();
    let mut sweep = end_angle - start_angle;
    if sweep < 0.0 { sweep += 2.0 * std::f64::consts::PI; }
    let segments = (sweep / SMALLEST_ANGLE).ceil().max(1.0) as usize;
    let step = sweep / segments as f64;
    let basis = Matrix3::arbitrary_axis(normal);
    for i in 0..=segments {
        let angle = start_angle + i as f64 * step;
        let local = Vector3::new(radius * angle.cos(), radius * angle.sin(), 0.0);
        pts.push(center + basis * local);
    }
    pts
}

fn tessellate_bulge(start: Vector2, end: Vector2, bulge: f64) -> Vec<Vector3> {
    let mut pts = Vec::new();
    let b = 0.5 * (1.0 / bulge - bulge);
    let direct = if bulge >= 0.0 { 1.0 } else { -1.0 };
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let l = (dx * dx + dy * dy).sqrt();
    let r1 = (0.5 * l * bulge).abs();
    let r2 = (0.5 * l * b).abs();
    let radius = r1 + r2;
    let cx = 0.5 * ((start.x + end.x) - b * (end.y - start.y));
    let cy = 0.5 * ((start.y + end.y) + b * (end.x - start.x));
    let total_angle = 4.0 * bulge.abs().atan();
    let start_angle = (start.y - cy).atan2(start.x - cx);
    let segments = (total_angle / SMALLEST_ANGLE).ceil().max(1.0) as usize;
    let step = total_angle / segments as f64 * direct;
    pts.push(Vector3::new(start.x, start.y, 0.0));
    for i in 1..segments {
        let a = start_angle + i as f64 * step;
        pts.push(Vector3::new(cx + radius * a.cos(), cy + radius * a.sin(), 0.0));
    }
    pts.push(Vector3::new(end.x, end.y, 0.0));
    pts
}

// ═══════════════════════════════════════════════════════════════
//  实体 → GeoJSON Feature 转换
// ═══════════════════════════════════════════════════════════════

fn line_to_feature(line: &Line) -> Value {
    let color = color_to_rgb_string(line.common.color);
    make_feature_with_code("LineString", json!([pt(line.start), pt(line.end)]), json!({"color": color}), line.common.handle.value())
}

fn point_to_feature(point: &Point) -> Value {
    let color = color_to_rgb_string(point.common.color);
    let wcs = ocs_to_wcs(point.normal, point.location);
    make_feature_with_code("Point", pt(wcs), json!({"color": color}), point.common.handle.value())
}

fn circle_to_feature(circle: &Circle) -> Value {
    let color = color_to_rgb_string(circle.common.color);
    let center = ocs_to_wcs(circle.normal, circle.center);
    let mut coords = Vec::new();
    let segments = 60;
    for i in 0..=segments {
        let angle = i as f64 * 2.0 * std::f64::consts::PI / segments as f64;
        let local = Vector3::new(circle.radius * angle.cos(), circle.radius * angle.sin(), 0.0);
        let wcs_pt = center + Matrix3::arbitrary_axis(circle.normal) * local;
        coords.push(pt(wcs_pt));
    }
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), circle.common.handle.value())
}

fn arc_to_feature(arc: &Arc) -> Value {
    let color = color_to_rgb_string(arc.common.color);
    let mut coords = Vec::new();
    let mut sweep = arc.end_angle - arc.start_angle;
    if sweep < 0.0 { sweep += 2.0 * std::f64::consts::PI; }
    let segments = (sweep / SMALLEST_ANGLE).ceil().max(1.0) as usize;
    let step = sweep / segments as f64;
    let basis = Matrix3::arbitrary_axis(arc.normal);
    for i in 0..=segments {
        let angle = arc.start_angle + i as f64 * step;
        let local = Vector3::new(arc.radius * angle.cos(), arc.radius * angle.sin(), 0.0);
        let wcs_pt = basis * (arc.center + local);
        coords.push(pt(wcs_pt));
    }
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), arc.common.handle.value())
}

fn text_to_feature(text: &Text) -> Value {
    let color = color_to_rgb_string(text.common.color);
    let ref_point = text.alignment_point.unwrap_or(text.insertion_point);
    let wcs = ocs_to_wcs(text.normal, ref_point);
    let rotation_deg = calc_text_rotation(text.rotation, text.normal);
    let plain = acadrust::entities::mtext_format::parse_plain_text(&text.value);
    let display_text = plain.to_plain_text();
    let h_align = match text.horizontal_alignment {
        acadrust::entities::text::TextHorizontalAlignment::Left => "left",
        acadrust::entities::text::TextHorizontalAlignment::Center => "center",
        acadrust::entities::text::TextHorizontalAlignment::Right => "right",
        acadrust::entities::text::TextHorizontalAlignment::Middle => "middle",
        _ => "left",
    };
    let v_align = match text.vertical_alignment {
        acadrust::entities::text::TextVerticalAlignment::Baseline => "baseline",
        acadrust::entities::text::TextVerticalAlignment::Bottom => "bottom",
        acadrust::entities::text::TextVerticalAlignment::Middle => "middle",
        acadrust::entities::text::TextVerticalAlignment::Top => "top",
    };
    make_feature_with_code("Point", pt(wcs), json!({
        "color": color, "text": display_text, "fontSize": text.height,
        "rotation": rotation_deg, "textAlign": h_align, "textBaseline": v_align
    }), text.common.handle.value())
}

fn mtext_to_feature(mtext: &MText) -> Value {
    let color = color_to_rgb_string(mtext.common.color);
    let wcs = ocs_to_wcs(mtext.normal, mtext.insertion_point);
    let rotation_deg = calc_text_rotation(mtext.rotation, mtext.normal);
    let doc = acadrust::entities::mtext_format::parse_mtext(&mtext.value, true);
    let display_text = doc.to_plain_text();
    let alignment = mtext.attachment_point as i32;
    make_feature_with_code("Point", pt(wcs), json!({
        "color": color, "text": display_text, "fontSize": mtext.height,
        "rotation": rotation_deg, "align": alignment
    }), mtext.common.handle.value())
}

fn attrib_to_feature(attrib: &AttributeEntity) -> Value {
    let color = color_to_rgb_string(attrib.common.color);
    let is_default_align = matches!(
        (attrib.horizontal_alignment, attrib.vertical_alignment),
        (acadrust::entities::attribute_definition::HorizontalAlignment::Left,
         acadrust::entities::attribute_definition::VerticalAlignment::Baseline)
    );
    let ref_point = if is_default_align { attrib.insertion_point } else { attrib.alignment_point };
    let wcs = ocs_to_wcs(attrib.normal, ref_point);
    let rotation_deg = calc_text_rotation(attrib.rotation, attrib.normal);
    let h_align = match attrib.horizontal_alignment {
        acadrust::entities::attribute_definition::HorizontalAlignment::Left => "left",
        acadrust::entities::attribute_definition::HorizontalAlignment::Center => "center",
        acadrust::entities::attribute_definition::HorizontalAlignment::Right => "right",
        acadrust::entities::attribute_definition::HorizontalAlignment::Middle => "middle",
        _ => "left",
    };
    let v_align = match attrib.vertical_alignment {
        acadrust::entities::attribute_definition::VerticalAlignment::Baseline => "baseline",
        acadrust::entities::attribute_definition::VerticalAlignment::Bottom => "bottom",
        acadrust::entities::attribute_definition::VerticalAlignment::Middle => "middle",
        acadrust::entities::attribute_definition::VerticalAlignment::Top => "top",
    };
    make_feature_with_code("Point", pt(wcs), json!({
        "color": color, "text": attrib.value, "fontSize": attrib.height,
        "rotation": rotation_deg, "textAlign": h_align, "textBaseline": v_align
    }), attrib.common.handle.value())
}

fn lwpolyline_to_features(pl: &LwPolyline) -> Vec<Value> {
    let color = color_to_rgb_string(pl.common.color);
    let mut features = Vec::new();
    let verts = &pl.vertices;
    let n = verts.len();
    if n == 0 {
        features.push(make_feature("LineString", Value::Array(vec![]), json!({"color": color})));
        return features;
    }
    let normal = pl.normal;
    let basis = Matrix3::arbitrary_axis(normal);

    let mut coords: Vec<Value> = Vec::new();
    for i in 0..n {
        let v = &verts[i];
        let start = v.location;
        let bulge = v.bulge;
        if 4.0 * bulge.abs().atan() / std::f64::consts::PI * 180.0 >= 7.0 && i < n - 1 {
            let next = &verts[(i + 1) % n];
            let end = next.location;
            let arc_pts = tessellate_bulge(start, end, bulge);
            for p in &arc_pts {
                let wcs_pt = basis * Vector3::new(p.x, p.y, pl.elevation);
                coords.push(pt(wcs_pt));
            }
        } else {
            let wcs_pt = basis * Vector3::new(start.x, start.y, pl.elevation);
            coords.push(pt(wcs_pt));
        }
    }
    if pl.is_closed && coords.len() > 1 { coords.push(coords[0].clone()); }

    let mut props = Map::new();
    props.insert("color".into(), json!(color));
    let has_widths = verts.iter().any(|v| v.start_width != 0.0 || v.end_width != 0.0);
    if has_widths {
        let widths: Vec<Value> = verts.iter().map(|v| json!([v.start_width, v.end_width])).collect();
        props.insert("widths".into(), Value::Array(widths));
    }
    if pl.constant_width != 0.0 { props.insert("constantWidth".into(), json!(pl.constant_width)); }
    features.push(make_feature_with_code("LineString", Value::Array(coords), Value::Object(props), pl.common.handle.value()));

    // 箭头段 → 闭合填充 Polygon
    for i in 0..n.saturating_sub(1) {
        let v0 = &verts[i];
        let v1 = &verts[i + 1];
        let sw = v0.start_width;
        let ew = v0.end_width;
        if (sw - ew).abs() < 1e-6 || (sw == 0.0 && ew == 0.0) { continue; }
        let p0 = v0.location;
        let p1 = v1.location;
        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len < 1e-10 { continue; }
        let nx = -dy / seg_len;
        let ny = dx / seg_len;
        let corners_2d: Vec<Vector2> = if sw > ew {
            let half_sw = sw / 2.0;
            vec![
                Vector2::new(p0.x + half_sw * nx, p0.y + half_sw * ny),
                Vector2::new(p0.x - half_sw * nx, p0.y - half_sw * ny),
                p1,
            ]
        } else {
            let half_ew = ew / 2.0;
            vec![
                Vector2::new(p1.x + half_ew * nx, p1.y + half_ew * ny),
                Vector2::new(p1.x - half_ew * nx, p1.y - half_ew * ny),
                p0,
            ]
        };
        let corners_wcs: Vec<Vector3> = corners_2d.iter()
            .map(|c| basis * Vector3::new(c.x, c.y, pl.elevation))
            .collect();
        let mut ring: Vec<Value> = corners_wcs.iter().map(|c| pt(*c)).collect();
        ring.push(pt(corners_wcs[0]));
        features.push(make_feature_with_code("Polygon", json!([ring]), json!({
            "color": color, "arrow": true, "segment": i, "startWidth": sw, "endWidth": ew,
        }), pl.common.handle.value()));
    }
    features
}

fn polyline3d_to_feature(pl: &Polyline) -> Value {
    let color = color_to_rgb_string(pl.common.color);
    let coords: Vec<Value> = pl.vertices.iter().map(|v| pt(v.location)).collect();
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), pl.common.handle.value())
}

fn polyline2d_to_feature(pl: &Polyline2D) -> Value {
    let color = color_to_rgb_string(pl.common.color);
    let mut coords: Vec<Value> = Vec::new();
    let verts = &pl.vertices;
    let n = verts.len();
    let basis = Matrix3::arbitrary_axis(pl.normal);
    for i in 0..n {
        let v = &verts[i];
        let start = v.location;
        let bulge = v.bulge;
        if 4.0 * bulge.abs().atan() / std::f64::consts::PI * 180.0 >= 7.0 && i < n - 1 {
            let next = &verts[(i + 1) % n];
            let end = next.location;
            let arc_pts = tessellate_bulge(Vector2::new(start.x, start.y), Vector2::new(end.x, end.y), bulge);
            for p in &arc_pts {
                let wcs_pt = basis * Vector3::new(p.x, p.y, pl.elevation);
                coords.push(pt(wcs_pt));
            }
        } else {
            let wcs_pt = basis * Vector3::new(start.x, start.y, pl.elevation);
            coords.push(pt(wcs_pt));
        }
    }
    if pl.is_closed() && coords.len() > 1 { coords.push(coords[0].clone()); }
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), pl.common.handle.value())
}

fn ellipse_to_feature(ellipse: &Ellipse) -> Value {
    let color = color_to_rgb_string(ellipse.common.color);
    let center = ellipse.center;
    let major = ellipse.major_axis;
    let major_len = major.length();
    let minor_len = major_len * ellipse.minor_axis_ratio;
    let u = major * (1.0 / major_len.max(1e-12));
    let v = ellipse.normal.cross(&u);
    let start_param = ellipse.start_parameter;
    let mut end_param = ellipse.end_parameter;
    if end_param <= start_param { end_param += 2.0 * std::f64::consts::PI; }
    let delta = 0.05;
    let mut coords = Vec::new();
    let mut t = start_param;
    while t <= end_param + delta {
        if t > end_param { t = end_param; }
        let p = center + u * (major_len * t.cos()) + v * (minor_len * t.sin());
        coords.push(pt(p));
        if (t - end_param).abs() < 1e-12 { break; }
        t += delta;
    }
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), ellipse.common.handle.value())
}

fn spline_to_feature(spline: &Spline) -> Value {
    let color = color_to_rgb_string(spline.common.color);
    let pts = evaluate_nurbs(spline);
    let coords: Vec<Value> = pts.iter().map(|p| pt(*p)).collect();
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), spline.common.handle.value())
}

fn evaluate_nurbs(spline: &Spline) -> Vec<Vector3> {
    let cps = &spline.control_points; let knots = &spline.knots;
    let weights = &spline.weights; let degree = spline.degree as usize;
    if cps.is_empty() || knots.is_empty() || degree == 0 { return cps.clone(); }
    let n = cps.len(); let use_weights = weights.len() == n;
    let t_min = if degree < knots.len() { knots[degree] } else { 0.0 };
    let t_max = if n < knots.len() { knots[n] } else { knots.last().copied().unwrap_or(1.0) };
    let num_samples = ((n * 10).max(20)).min(1000);
    let step = (t_max - t_min) / num_samples as f64;
    let mut result = Vec::with_capacity(num_samples + 1);
    for i in 0..=num_samples {
        let t = (t_min + i as f64 * step).min(t_max);
        let mut span = degree;
        for k in degree..knots.len().saturating_sub(1) {
            if t >= knots[k] && t < knots[k + 1] { span = k; break; }
            if k == knots.len() - 2 { span = k; }
        }
        let mut d: Vec<Vector3> = Vec::with_capacity(degree + 1);
        let mut w: Vec<f64> = Vec::with_capacity(degree + 1);
        for j in 0..=degree {
            let idx = if span >= degree && span - degree + j < n { span - degree + j } else { j.min(n - 1) };
            d.push(cps[idx]); w.push(if use_weights { weights[idx] } else { 1.0 });
        }
        for r in 1..=degree {
            for j in (r..=degree).rev() {
                let idx = span - degree + j;
                let left = if idx > 0 && idx <= knots.len() - 1 { knots[idx] } else { t };
                let right = if idx + degree - r + 1 < knots.len() { knots[idx + degree - r + 1] } else { t };
                let denom = right - left;
                let alpha = if denom.abs() < 1e-14 { 0.0 } else { (t - left) / denom };
                let prev = j - 1;
                d[j] = d[prev] * (1.0 - alpha) + d[j] * alpha;
                w[j] = w[prev] * (1.0 - alpha) + w[j] * alpha;
            }
        }
        let wf = w[degree];
        if wf.abs() > 1e-14 { result.push(d[degree] * (1.0 / wf)); } else { result.push(d[degree]); }
    }
    result
}

fn hatch_to_feature(hatch: &Hatch) -> Value {
    let color = color_to_rgb_string(hatch.common.color);
    let mut polygon_coords: Vec<Value> = Vec::new();
    let basis = Matrix3::arbitrary_axis(hatch.normal);
    for path in &hatch.paths {
        let mut ring: Vec<Value> = Vec::new();
        for edge in &path.edges {
            match edge {
                BoundaryEdge::Line(le) => {
                    let s = basis * Vector3::new(le.start.x, le.start.y, 0.0);
                    let e = basis * Vector3::new(le.end.x, le.end.y, 0.0);
                    if ring.is_empty() { ring.push(pt(s)); } ring.push(pt(e));
                }
                BoundaryEdge::CircularArc(ae) => {
                    let c = ae.center; let r = ae.radius;
                    let mut sa = ae.start_angle; let mut ea = ae.end_angle;
                    if ea <= sa { ea += 2.0 * std::f64::consts::PI; }
                    if !ae.counter_clockwise { std::mem::swap(&mut sa, &mut ea); ea += 2.0 * std::f64::consts::PI; }
                    let sweep = ea - sa; let seg = (sweep / SMALLEST_ANGLE).ceil().max(1.0) as usize; let step = sweep / seg as f64;
                    if ring.is_empty() { ring.push(pt(basis * (Vector3::new(c.x, c.y, 0.0) + Vector3::new(r * sa.cos(), r * sa.sin(), 0.0)))); }
                    for i in 1..=seg { let a = sa + i as f64 * step;
                        ring.push(pt(basis * (Vector3::new(c.x, c.y, 0.0) + Vector3::new(r * a.cos(), r * a.sin(), 0.0)))); }
                }
                BoundaryEdge::EllipticArc(ee) => {
                    let c = ee.center; let me = ee.major_axis_endpoint;
                    let ml = me.length(); let ml2 = ml * ee.minor_axis_ratio;
                    let u = Vector3::new(me.x, me.y, 0.0) * (1.0 / ml.max(1e-12));
                    let v = Vector3::new(-u.y, u.x, 0.0);
                    let sa = ee.start_angle; let mut ea = ee.end_angle;
                    if ea <= sa { ea += 2.0 * std::f64::consts::PI; }
                    let sweep = ea - sa; let seg = (sweep / 0.05).ceil().max(1.0) as usize; let step = sweep / seg as f64;
                    if ring.is_empty() { ring.push(pt(basis * (Vector3::new(c.x, c.y, 0.0) + u * (ml * sa.cos()) + v * (ml2 * sa.sin())))); }
                    for i in 1..=seg { let t = sa + i as f64 * step;
                        ring.push(pt(basis * (Vector3::new(c.x, c.y, 0.0) + u * (ml * t.cos()) + v * (ml2 * t.sin())))); }
                }
                BoundaryEdge::Polyline(pe) => {
                    let vs = &pe.vertices; let nv = vs.len();
                    for i in 0..nv { let v = &vs[i]; let bulge = v.z; let s = Vector2::new(v.x, v.y);
                        if bulge.abs() > 1e-10 && i < nv - 1 {
                            let nx = &vs[(i + 1) % nv]; let e = Vector2::new(nx.x, nx.y);
                            for p in &tessellate_bulge(s, e, bulge) { ring.push(pt(basis * Vector3::new(p.x, p.y, 0.0))); }
                        } else { ring.push(pt(basis * Vector3::new(s.x, s.y, 0.0))); }
                    }
                    if pe.is_closed && ring.len() > 1 { ring.push(ring[0].clone()); }
                }
                BoundaryEdge::Spline(se) => { for cp in &se.control_points { ring.push(pt(basis * Vector3::new(cp.x, cp.y, 0.0))); } }
            }
        }
        if ring.len() > 1 && ring.first() != ring.last() { ring.push(ring[0].clone()); }
        if !ring.is_empty() { polygon_coords.push(Value::Array(vec![Value::Array(ring)])); }
    }
    make_feature_with_code("MultiPolygon", Value::Array(polygon_coords), json!({"color": color}), hatch.common.handle.value())
}

fn solid_to_feature(s: &Solid) -> Value {
    let c = color_to_rgb_string(s.common.color); let b = Matrix3::arbitrary_axis(s.normal);
    let p1 = b*s.first_corner; let p2 = b*s.second_corner; let p3 = b*s.third_corner; let p4 = b*s.fourth_corner;
    let mut r = vec![pt(p1), pt(p2), pt(p3)];
    if !s.is_triangle() { r.push(pt(p4)); }
    r.push(r[0].clone());
    make_feature_with_code("Polygon", json!([r]), json!({"color": c}), s.common.handle.value())
}

fn face3d_to_feature(f: &Face3D) -> Value {
    let c = color_to_rgb_string(f.common.color);
    let mut r = vec![pt(f.first_corner), pt(f.second_corner), pt(f.third_corner)];
    if !f.is_triangle() { r.push(pt(f.fourth_corner)); }
    r.push(r[0].clone());
    make_feature_with_code("Polygon", json!([r]), json!({"color": c}), f.common.handle.value())
}

fn dimension_to_features(dim: &Dimension) -> Vec<Value> {
    let base = dim.base(); let color = color_to_rgb_string(base.common.color);
    let handle = base.common.handle.value(); let mut features = Vec::new();
    let lc: Vec<Value> = match dim {
        Dimension::Linear(d) => vec![pt(d.first_point), pt(d.definition_point), pt(d.second_point)],
        Dimension::Aligned(d) => vec![pt(d.first_point), pt(d.definition_point), pt(d.second_point)],
        Dimension::Radius(d) => vec![pt(d.angle_vertex), pt(d.definition_point)],
        Dimension::Diameter(d) => vec![pt(d.angle_vertex), pt(d.definition_point)],
        Dimension::Angular2Ln(d) => vec![pt(d.first_point), pt(d.angle_vertex), pt(d.second_point), pt(d.dimension_arc)],
        Dimension::Angular3Pt(d) => vec![pt(d.first_point), pt(d.angle_vertex), pt(d.second_point)],
        Dimension::Ordinate(d) => vec![pt(d.feature_location), pt(d.leader_endpoint)],
    };
    if lc.len() >= 2 { features.push(make_feature_with_code("LineString", Value::Array(lc), json!({"color": color}), handle)); }
    let dt = if !base.text.is_empty() { base.text.clone() }
        else if let Some(ref ut) = base.user_text { ut.clone() }
        else { format!("{:.2}", base.actual_measurement) };
    let ds = match dim { Dimension::Linear(_) => "linear", Dimension::Aligned(_) => "aligned",
        Dimension::Radius(_) => "radius", Dimension::Diameter(_) => "diameter",
        Dimension::Angular2Ln(_) => "angular", Dimension::Angular3Pt(_) => "angular3pt", Dimension::Ordinate(_) => "ordinate" };
    let rot = calc_text_rotation(base.text_rotation, base.normal);
    features.push(make_feature_with_code("Point", pt(base.text_middle_point), json!({
        "color": color, "text": dt, "fontSize": 0.0, "rotation": rot,
        "measurement": base.actual_measurement, "dimensionType": ds }), handle));
    features
}

fn helix_to_feature(h: &Helix) -> Value { spline_to_feature(&h.spline) }

fn has_empty_coords(f: &Value) -> bool {
    if let Some(g) = f.get("geometry") { if let Some(c) = g.get("coordinates") { return check_empty_coords(c); } return true; } true
}
fn check_empty_coords(v: &Value) -> bool {
    match v { Value::Array(a) => a.is_empty() || a.iter().any(check_empty_coords), _ => false }
}

fn read_crs_from_doc(doc: &CadDocument, _q: bool) -> Value {
    let def = json!({"type":"name","properties":{"name":"urn:ogc:def:crs:EPSG::900913"}});
    for obj in doc.objects.values() {
        if let ObjectType::GeoData(gd) = obj {
            if gd.coordinate_system_definition.is_empty() { continue; }
            if let Some(e) = extract_epsg_from_wkt(&gd.coordinate_system_definition) {
                return json!({"type":"name","properties":{"name":format!("urn:ogc:def:crs:EPSG::{}",e)}}); }
            if let Some(n) = extract_crs_from_xml(&gd.coordinate_system_definition) {
                return json!({"type":"name","properties":{"name":n}}); }
        }
    }
    def
}
fn extract_epsg_from_wkt(wkt: &str) -> Option<String> {
    let w = wkt.to_uppercase(); let p = "AUTHORITY[\"EPSG\",\"";
    if let Some(pos) = w.find(p) { let s = pos + p.len(); if let Some(e) = wkt[s..].find('"') { return Some(wkt[s..s+e].to_string()); } } None
}
fn extract_crs_from_xml(xml: &str) -> Option<String> {
    if let Some(c) = xtag(xml, "EPSG_CODE") { return Some(format!("urn:ogc:def:crs:EPSG::{}",c)); }
    if let Some(n) = xtag(xml, "CS_NAME") { return Some(format!("urn:ogc:def:crs:EPSG::{}",n)); } None
}
fn xtag(xml: &str, tag: &str) -> Option<String> {
    let o = format!("<{}>", tag); let c = format!("</{}>", tag);
    if let Some(s) = xml.find(&o) { let vs = s + o.len(); if let Some(e) = xml[vs..].find(&c) { return Some(xml[vs..vs+e].trim().to_string()); } } None
}

fn collect_positioning_baseline_features(doc: &CadDocument, layer_name: &str) -> Vec<Value> {
    let mut features = Vec::new();
    struct TI { pos: Vector3, text: String }
    struct LE { verts: Vec<Vector3>, feats: Vec<Value> }
    let mut texts: Vec<TI> = Vec::new();
    let mut lines: Vec<LE> = Vec::new();
    for e in doc.entities().filter(|e| e.common().layer == layer_name) {
        match e {
            EntityType::Text(t) => { let rp = t.alignment_point.unwrap_or(t.insertion_point);
                texts.push(TI { pos: ocs_to_wcs(t.normal, rp), text: acadrust::entities::mtext_format::parse_plain_text(&t.value).to_plain_text() }); }
            EntityType::MText(t) => {
                texts.push(TI { pos: ocs_to_wcs(t.normal, t.insertion_point), text: acadrust::entities::mtext_format::parse_mtext(&t.value, true).to_plain_text() }); }
            EntityType::Line(l) => { lines.push(LE { verts: vec![l.start, l.end], feats: vec![line_to_feature(l)] }); }
            EntityType::LwPolyline(p) => { let f = lwpolyline_to_features(p); let b = Matrix3::arbitrary_axis(p.normal);
                lines.push(LE { verts: p.vertices.iter().map(|v| b * Vector3::new(v.location.x, v.location.y, p.elevation)).collect(), feats: f }); }
            EntityType::Polyline2D(p) => { let f = polyline2d_to_feature(p); let b = Matrix3::arbitrary_axis(p.normal);
                lines.push(LE { verts: p.vertices.iter().map(|v| b * Vector3::new(v.location.x, v.location.y, p.elevation)).collect(), feats: vec![f] }); }
            _ => {}
        }
    }
    let mut avail: Vec<(usize, &TI)> = texts.iter().enumerate().collect();
    for le in &lines {
        let mut md = f64::MAX; let mut np: Option<usize> = None;
        for (pos, (_, ti)) in avail.iter().enumerate() {
            for seg in le.verts.windows(2) { let d = calc_h(ti.pos, seg[0], seg[1]); if d < md { md = d; np = Some(pos); } }
        }
        let mut af = le.feats.clone();
        if let Some(pos) = np { if md < 2.0 {
            let (_, info) = avail[pos];
            if let Some(mf) = af.first_mut() { if let Some(p) = mf.get_mut("properties").and_then(|p| p.as_object_mut()) { p.insert("tunnelCode".into(), json!(info.text)); } }
            avail.remove(pos);
        } }
        features.extend(af);
    }
    features
}
fn calc_h(tn: Vector3, p1: Vector3, p2: Vector3) -> f64 {
    let a = d2d(p1, p2); if a == 0.0 { return d2d(p1, tn); }
    let b = d2d(p1, tn); let c = d2d(p2, tn);
    if (c+b-a).abs() < 1e-6 { return 0.0; }
    if a*a+b*b >= c*c && a*a+c*c >= b*b { let p = (a+b+c)/2.0; let s = (p*(p-a)*(p-b)*(p-c)).abs().sqrt(); return 2.0*s/a; }
    f64::MAX
}
fn d2d(a: Vector3, b: Vector3) -> f64 { let dx = a.x-b.x; let dy = a.y-b.y; (dx*dx+dy*dy).sqrt() }

fn calc_text_rotation(rot: f64, n: Vector3) -> f64 {
    if n.x.abs() < 1.0/64.0 && n.y.abs() < 1.0/64.0 { return rot.to_degrees(); }
    let nn = vn3(n.x, n.y, n.z);
    let rv = if nn.0.abs() < 1.0/64.0 && nn.1.abs() < 1.0/64.0 { (0.0,1.0,0.0) } else { (0.0,0.0,1.0) };
    let u = vnn(vcross(rv, nn)); let v = vcross(nn, u);
    let vw = (rot.cos()*u.0+rot.sin()*v.0, rot.cos()*u.1+rot.sin()*v.1, rot.cos()*u.2+rot.sin()*v.2);
    let mut d = vw.1.atan2(vw.0).to_degrees(); if d < 0.0 { d += 360.0; } d
}
fn vn3(x: f64, y: f64, z: f64) -> (f64,f64,f64) { let l = (x*x+y*y+z*z).sqrt(); if l < 1e-12 { return (0.0,0.0,0.0); } (x/l,y/l,z/l) }
fn vcross(a: (f64,f64,f64), b: (f64,f64,f64)) -> (f64,f64,f64) { (a.1*b.2-a.2*b.1, a.2*b.0-a.0*b.2, a.0*b.1-a.1*b.0) }
fn vnn(v: (f64,f64,f64)) -> (f64,f64,f64) { vn3(v.0, v.1, v.2) }
