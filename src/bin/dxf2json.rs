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
use acadrust::tables::DimStyle;
use acadrust::types::{Color, Matrix3, Vector2, Vector3};
use acadrust::{CadDocument, EntityType};
#[cfg(not(feature = "server"))]
use acadrust::{DwgReader, DxfReader};
#[cfg(not(feature = "server"))]
use clap::Parser;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
#[cfg(not(feature = "server"))]
use std::fs;
#[cfg(not(feature = "server"))]
use std::path::Path;

// ── CLI 参数定义 ──────────────────────────────────────────────

/// DXF/DWG to GeoJSON 转换器
#[cfg(not(feature = "server"))]
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

// ═══════════════════════════════════════════════════════════════
//  Public API (shared with dxf2json-server via #[path])
// ═══════════════════════════════════════════════════════════════

/// Convert a CadDocument into a list of (layer_name, FeatureCollection JSON) pairs.
pub fn convert_document(
    doc: &CadDocument,
    filter_layers: &[String],
    merge_xref: bool,
) -> Vec<(String, Value)> {
    // 1. Explode Insert entities (model space only)
    let ms_handle = doc.header.model_space_block_handle;
    let mut exploded_by_layer: HashMap<String, Vec<EntityType>> = HashMap::new();
    for entity in doc
        .entities()
        .filter(|e| e.common().owner_handle == ms_handle)
    {
        if let EntityType::Insert(ins) = entity {
            let insert_color = resolve_insert_color(ins, doc);
            let mut exploded = ins.explode_from_document(doc);
            for sub in &mut exploded {
                if sub.common().color == Color::ByBlock {
                    sub.common_mut().color = insert_color;
                }
            }
            for sub_entity in exploded {
                let layer = sub_entity.common().layer.clone();
                exploded_by_layer.entry(layer).or_default().push(sub_entity);
            }
            for attrib in &ins.attributes {
                let mut a = attrib.clone();
                if a.common.color == Color::ByBlock {
                    a.common.color = insert_color;
                }
                let layer = a.common.layer.clone();
                exploded_by_layer
                    .entry(layer)
                    .or_default()
                    .push(EntityType::AttributeEntity(a));
            }
        }
    }

    // 2. Determine layers to process (skip off/frozen layers)
    let all_layer_names: Vec<String> = if filter_layers.is_empty() {
        doc.layers
            .iter()
            .filter(|l| l.is_visible())
            .map(|l| l.name.clone())
            .collect()
    } else {
        filter_layers.to_vec()
    };

    // 3. Group XREF variants if requested
    let layer_groups: Vec<(String, Vec<String>)> = if merge_xref {
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for name in &all_layer_names {
            let base = xref_base_name(name).to_string();
            if !groups.contains_key(&base) {
                order.push(base.clone());
            }
            groups.entry(base).or_default().push(name.clone());
        }
        order
            .into_iter()
            .map(|b| {
                let variants = groups.remove(&b).unwrap_or_default();
                (b, variants)
            })
            .collect()
    } else {
        all_layer_names
            .into_iter()
            .map(|n| (n.clone(), vec![n]))
            .collect()
    };

    // 4. Convert each layer group to a FeatureCollection
    let crs = read_crs_from_doc(doc, true);
    let mut results = Vec::new();
    for (output_name, variants) in &layer_groups {
        let mut features = Vec::new();
        for variant in variants {
            features.extend(collect_layer_features(doc, variant, &exploded_by_layer));
        }
        if features.is_empty() {
            continue;
        }

        let layer = doc
            .layers
            .get(output_name.as_str())
            .or_else(|| variants.iter().find_map(|v| doc.layers.get(v.as_str())));
        let visible = layer.map(|l| !l.flags.off).unwrap_or(true);
        let is_base_layer = output_name == "定位基准线";

        let mut fc = Map::new();
        fc.insert("type".into(), json!("FeatureCollection"));
        fc.insert("layerName".into(), json!(output_name));
        fc.insert("visible".into(), json!(if visible { "1" } else { "0" }));
        fc.insert(
            "layerType".into(),
            json!(if is_base_layer { "1" } else { "0" }),
        );
        if let Some(l) = layer {
            fc.insert("layerCode".into(), json!(format!("L{}", l.handle.value())));
        }
        fc.insert("crs".into(), crs.clone());

        let filtered: Vec<Value> = features
            .into_iter()
            .filter(|f| !has_empty_coords(f))
            .collect();
        fc.insert("features".into(), Value::Array(filtered));

        results.push((output_name.clone(), Value::Object(fc)));
    }
    results
}

// ═══════════════════════════════════════════════════════════════
//  CLI entry points
// ═══════════════════════════════════════════════════════════════

#[cfg(not(feature = "server"))]
fn main() -> acadrust::Result<()> {
    let cli = Cli::parse();

    // 创建输出目录
    fs::create_dir_all(&cli.output).map_err(|e| acadrust::error::DxfError::Io(e))?;

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
#[cfg(not(feature = "server"))]
fn process_file(input_file: &str, cli: &Cli) -> acadrust::Result<()> {
    // 1. 读取文件
    let ext = Path::new(input_file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("dxf")
        .to_lowercase();

    let doc = if ext == "dwg" {
        if !cli.quiet {
            println!("Reading DWG: {}", input_file);
        }
        let mut reader = DwgReader::from_file(input_file)?;
        reader.read()?
    } else {
        if !cli.quiet {
            println!("Reading DXF: {}", input_file);
        }
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

    // 2. 展开所有 Insert（块引用）——仅模型空间的 Insert
    let ms_handle = doc.header.model_space_block_handle;
    let mut exploded_by_layer: std::collections::HashMap<String, Vec<EntityType>> =
        std::collections::HashMap::new();

    for entity in doc
        .entities()
        .filter(|e| e.common().owner_handle == ms_handle)
    {
        if let EntityType::Insert(ins) = entity {
            let insert_color = resolve_insert_color(ins, &doc);
            let mut exploded = ins.explode_from_document(&doc);
            for sub in &mut exploded {
                if sub.common().color == Color::ByBlock {
                    sub.common_mut().color = insert_color;
                }
            }
            for sub_entity in exploded {
                let layer = sub_entity.common().layer.clone();
                exploded_by_layer.entry(layer).or_default().push(sub_entity);
            }
            for attrib in &ins.attributes {
                let mut a = attrib.clone();
                if a.common.color == Color::ByBlock {
                    a.common.color = insert_color;
                }
                let layer = a.common.layer.clone();
                exploded_by_layer
                    .entry(layer)
                    .or_default()
                    .push(EntityType::AttributeEntity(a));
            }
        }
    }

    let exploded_by_layer = exploded_by_layer;

    // 3. 确定要处理的图层（跳过关闭/冻结图层）
    let all_layer_names: Vec<String> = if cli.layers.is_empty() {
        doc.layers
            .iter()
            .filter(|l| l.is_visible())
            .map(|l| l.name.clone())
            .collect()
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
        order
            .into_iter()
            .map(|b| {
                let variants = groups.remove(&b).unwrap_or_default();
                (b, variants)
            })
            .collect()
    } else {
        all_layer_names
            .into_iter()
            .map(|n| (n.clone(), vec![n]))
            .collect()
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
        let layer = doc
            .layers
            .get(output_name.as_str())
            .or_else(|| variants.iter().find_map(|v| doc.layers.get(v.as_str())));
        let visible = layer.map(|l| !l.flags.off).unwrap_or(true);
        let is_base_layer = output_name == "定位基准线";

        let mut fc = Map::new();
        fc.insert("type".into(), json!("FeatureCollection"));
        fc.insert("layerName".into(), json!(output_name));
        fc.insert("visible".into(), json!(if visible { "1" } else { "0" }));
        fc.insert(
            "layerType".into(),
            json!(if is_base_layer { "1" } else { "0" }),
        );

        if let Some(l) = layer {
            fc.insert("layerCode".into(), json!(format!("L{}", l.handle.value())));
        }

        let crs = read_crs_from_doc(&doc, cli.quiet);
        fc.insert("crs".into(), crs);

        let filtered: Vec<Value> = features
            .into_iter()
            .filter(|f| !has_empty_coords(f))
            .collect();
        fc.insert("features".into(), Value::Array(filtered));

        let safe_name = output_name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        let output_file = format!("{}/{}.json", cli.output, safe_name);
        fs::write(
            &output_file,
            serde_json::to_string_pretty(&Value::Object(fc))
                .map_err(|e| acadrust::error::DxfError::Custom(e.to_string()))?,
        )?;
        layer_count += 1;
        if !cli.quiet {
            println!("  Written: {}", output_file);
        }
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
        return collect_positioning_baseline_features(doc, layer_name, exploded_by_layer);
    }

    let mut features = Vec::new();

    for entity in doc.entities().filter(|e| {
        e.common().layer == layer_name
            && e.common().owner_handle == doc.header.model_space_block_handle
    }) {
        if matches!(
            entity,
            EntityType::Insert(_) | EntityType::AttributeDefinition(_)
        ) {
            continue;
        }
        if let Some(fs) = entity_to_features(entity, doc) {
            features.extend(fs);
        }
    }

    if let Some(exploded) = exploded_by_layer.get(layer_name) {
        for entity in exploded {
            if let Some(fs) = entity_to_features(entity, doc) {
                features.extend(fs);
            }
        }
    }

    features
}

fn entity_to_features(entity: &EntityType, doc: &CadDocument) -> Option<Vec<Value>> {
    let features = match entity {
        EntityType::Line(e) => vec![line_to_feature(e, doc)],
        EntityType::Point(e) => vec![point_to_feature(e, doc)],
        EntityType::Circle(e) => vec![circle_to_feature(e, doc)],
        EntityType::Arc(e) => vec![arc_to_feature(e, doc)],
        EntityType::Text(e) => vec![text_to_feature(e, doc)],
        EntityType::MText(e) => mtext_to_features(e, doc),
        EntityType::AttributeEntity(e) => vec![attrib_to_feature(e, doc)],
        EntityType::LwPolyline(e) => lwpolyline_to_features(e, doc),
        EntityType::Polyline(e) => vec![polyline3d_to_feature(e, doc)],
        EntityType::Polyline2D(e) => vec![polyline2d_to_feature(e, doc)],
        EntityType::Polyline3D(e) => vec![polyline3d_new_to_feature(e, doc)],
        EntityType::Ellipse(e) => vec![ellipse_to_feature(e, doc)],
        EntityType::Spline(e) => vec![spline_to_feature(e, doc)],
        EntityType::Hatch(e) => vec![hatch_to_feature(e, doc)],
        EntityType::Solid(e) => vec![solid_to_feature(e, doc)],
        EntityType::Face3D(e) => vec![face3d_to_feature(e, doc)],
        EntityType::Dimension(e) => dimension_to_features(e, doc),
        EntityType::Helix(e) => vec![helix_to_feature(e, doc)],
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

/// Resolve the effective color of an Insert entity.
/// ByLayer on the Insert itself falls back to its layer's color.
/// ByBlock on the Insert means "inherit from parent block" — at the
/// top level (model space) there is no parent, so it resolves to white (ACI 7).
fn resolve_insert_color(ins: &Insert, doc: &CadDocument) -> Color {
    match ins.common.color {
        Color::ByLayer => doc
            .layers
            .get(&ins.common.layer)
            .map(|l| l.color)
            .unwrap_or(Color::Index(7)),
        // ByBlock at top level → white (no parent block to inherit from)
        Color::ByBlock => Color::Index(7),
        c => c,
    }
}

fn resolve_color(color: Color, layer_name: &str, doc: &CadDocument) -> (u8, u8, u8) {
    match color {
        Color::Rgb { r, g, b } => (r, g, b),
        Color::Index(i) => Color::Index(i).rgb().unwrap_or((255, 255, 255)),
        // ByBlock: no parent block context → white (ACI 7)
        Color::ByBlock => (255, 255, 255),
        // ByLayer: resolve from layer table
        Color::ByLayer => doc
            .layers
            .get(layer_name)
            .and_then(|l| l.color.rgb())
            .unwrap_or((255, 255, 255)),
        acadrust::types::Color::None => (255, 255, 255),
    }
}

fn color_to_rgb_string(color: Color, layer_name: &str, doc: &CadDocument) -> String {
    let (r, g, b) = resolve_color(color, layer_name, doc);
    format!("{},{},{}", r, g, b)
}

/// Complex linetype element info (embedded text or shape)
struct LtComplexInfo {
    text: String,
    position: f64, // cumulative position within one pattern cycle
    scale: f64,
    rotation: f64,
    absolute_rotation: bool,
    offset: [f64; 2],
}

/// Extract linetype info: name, dash pattern, pattern length, and complex text elements
fn entity_linetype_info(
    common: &acadrust::entities::EntityCommon,
    doc: &CadDocument,
) -> (String, Option<Vec<f64>>, Option<f64>, Vec<LtComplexInfo>) {
    let lt_name = if common.has_linetype() {
        common.linetype.clone()
    } else {
        doc.layers
            .get(&common.layer)
            .map(|l| l.line_type.clone())
            .unwrap_or_else(|| "Continuous".to_string())
    };
    let lt = match doc.line_types.get(&lt_name) {
        Some(lt) if !lt.elements.is_empty() => lt,
        _ => return (lt_name, None, None, Vec::new()),
    };
    let pattern: Vec<f64> = lt.elements.iter().map(|e| e.length.abs()).collect();
    let pattern_length = lt.pattern_length;
    // Extract complex (text/shape) elements with their positions in the pattern
    let mut complex_list = Vec::new();
    let mut cumulative = 0.0;
    for elem in &lt.elements {
        if let Some(ref c) = elem.complex {
            if let Some(text) = c.text() {
                if !text.is_empty() {
                    complex_list.push(LtComplexInfo {
                        text: text.to_string(),
                        position: cumulative,
                        scale: c.scale,
                        rotation: c.rotation,
                        absolute_rotation: c.absolute_rotation,
                        offset: c.offset,
                    });
                }
            }
        }
        cumulative += elem.length.abs();
    }
    (lt_name, Some(pattern), Some(pattern_length), complex_list)
}

/// Resolve line weight to millimeters: entity → layer → default (0.25mm)
fn resolve_line_weight_mm(
    lw: &acadrust::types::LineWeight,
    layer_name: &str,
    doc: &CadDocument,
) -> f64 {
    use acadrust::types::LineWeight;
    match lw {
        LineWeight::Value(v) => *v as f64 / 100.0,
        LineWeight::ByLayer | LineWeight::ByBlock => {
            doc.layers
                .get(layer_name)
                .and_then(|l| l.line_weight.millimeters())
                .unwrap_or(0.25) // 默认线重 0.25mm
        }
        LineWeight::Default => 0.25,
    }
}

/// Build base properties: color + lineType + optional linePattern/linetypeScale/lineWeight
fn base_props(
    color: &str,
    common: &acadrust::entities::EntityCommon,
    doc: &CadDocument,
) -> Map<String, Value> {
    let (lt_name, lt_pattern, lt_pattern_len, lt_complex) = entity_linetype_info(common, doc);
    let mut props = Map::new();
    props.insert("color".into(), json!(color));
    props.insert("lineType".into(), json!(lt_name));
    if let Some(pat) = lt_pattern {
        props.insert(
            "linePattern".into(),
            Value::Array(pat.into_iter().map(|v| json!(v)).collect()),
        );
    }
    if let Some(plen) = lt_pattern_len {
        props.insert("linePatternLength".into(), json!(plen));
    }
    // 复杂线型嵌入文字（如管线标注 GAS）
    if !lt_complex.is_empty() {
        let arr: Vec<Value> = lt_complex
            .iter()
            .map(|c| {
                json!({
                    "text": c.text,
                    "position": c.position,
                    "scale": c.scale,
                    "rotation": c.rotation,
                    "absoluteRotation": c.absolute_rotation,
                    "offset": c.offset
                })
            })
            .collect();
        props.insert("lineTypeText".into(), Value::Array(arr));
    }
    // 实体级线型缩放因子，非 1.0 时输出
    if (common.linetype_scale - 1.0).abs() > 1e-9 {
        props.insert("linetypeScale".into(), json!(common.linetype_scale));
    }
    // 线重（mm）：实体 → 图层 → 默认(0.25mm)
    let lw_mm = resolve_line_weight_mm(&common.line_weight, &common.layer, doc);
    props.insert("lineWeight".into(), json!(lw_mm));
    props
}

fn make_feature(geom_type: &str, coordinates: Value, properties: Value) -> Value {
    json!({
        "type": "Feature",
        "geometry": { "type": geom_type, "coordinates": coordinates },
        "properties": properties
    })
}

fn make_feature_with_code(
    geom_type: &str,
    coordinates: Value,
    properties: Value,
    code: u64,
) -> Value {
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
    center: Vector3,
    radius: f64,
    start_angle: f64,
    end_angle: f64,
    normal: Vector3,
) -> Vec<Vector3> {
    let mut pts = Vec::new();
    let mut sweep = end_angle - start_angle;
    if sweep < 0.0 {
        sweep += 2.0 * std::f64::consts::PI;
    }
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
        pts.push(Vector3::new(
            cx + radius * a.cos(),
            cy + radius * a.sin(),
            0.0,
        ));
    }
    pts.push(Vector3::new(end.x, end.y, 0.0));
    pts
}

// ═══════════════════════════════════════════════════════════════
//  实体 → GeoJSON Feature 转换
// ═══════════════════════════════════════════════════════════════

fn line_to_feature(line: &Line, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(line.common.color, &line.common.layer, doc);
    make_feature_with_code(
        "LineString",
        json!([pt(line.start), pt(line.end)]),
        Value::Object(base_props(&color, &line.common, doc)),
        line.common.handle.value(),
    )
}

fn point_to_feature(point: &Point, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(point.common.color, &point.common.layer, doc);
    let wcs = ocs_to_wcs(point.normal, point.location);
    make_feature_with_code(
        "Point",
        pt(wcs),
        Value::Object(base_props(&color, &point.common, doc)),
        point.common.handle.value(),
    )
}

fn circle_to_feature(circle: &Circle, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(circle.common.color, &circle.common.layer, doc);
    let center = ocs_to_wcs(circle.normal, circle.center);
    let mut coords = Vec::new();
    // let segments = 60;
    // for i in 0..=segments {
    //     let angle = i as f64 * 2.0 * std::f64::consts::PI / segments as f64;
    //     let local = Vector3::new(
    //         circle.radius * angle.cos(),
    //         circle.radius * angle.sin(),
    //         0.0,
    //     );
    //     let wcs_pt = center + Matrix3::arbitrary_axis(circle.normal) * local;
    //     coords.push(pt(wcs_pt));
    // }
    coords.push(pt(center));
    let mut props = base_props(&color, &circle.common, doc);
    props.insert("radius".into(), json!(circle.radius));
    make_feature_with_code(
        "Circle",
        Value::Array(coords),
        Value::Object(props),
        circle.common.handle.value(),
    )
}

fn arc_to_feature(arc: &Arc, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(arc.common.color, &arc.common.layer, doc);
    let mut coords = Vec::new();
    let mut sweep = arc.end_angle - arc.start_angle;
    if sweep < 0.0 {
        sweep += 2.0 * std::f64::consts::PI;
    }
    let segments = (sweep / SMALLEST_ANGLE).ceil().max(1.0) as usize;
    let step = sweep / segments as f64;
    let basis = Matrix3::arbitrary_axis(arc.normal);
    for i in 0..=segments {
        let angle = arc.start_angle + i as f64 * step;
        let local = Vector3::new(arc.radius * angle.cos(), arc.radius * angle.sin(), 0.0);
        let wcs_pt = basis * (arc.center + local);
        coords.push(pt(wcs_pt));
    }
    make_feature_with_code(
        "LineString",
        Value::Array(coords),
        Value::Object(base_props(&color, &arc.common, doc)),
        arc.common.handle.value(),
    )
}

fn text_to_feature(text: &Text, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(text.common.color, &text.common.layer, doc);
    // Always use insertion_point as anchor (left/baseline position in DXF).
    // This avoids mismatches between CAD's vertical alignment semantics and
    // OpenLayers' Canvas textBaseline interpretation, which can cause text
    // overlap when different baselines (e.g. "baseline" vs "top") are mixed.
    let wcs = ocs_to_wcs(text.normal, text.insertion_point);
    let rotation_deg = calc_text_rotation(text.rotation, text.normal);
    let plain = acadrust::entities::mtext_format::parse_plain_text(&text.value);
    let display_text = plain.to_plain_text();
    let h_align = match text.horizontal_alignment {
        acadrust::entities::text::TextHorizontalAlignment::Left
        | acadrust::entities::text::TextHorizontalAlignment::Aligned
        | acadrust::entities::text::TextHorizontalAlignment::Fit => "left",
        acadrust::entities::text::TextHorizontalAlignment::Center
        | acadrust::entities::text::TextHorizontalAlignment::Middle => "center",
        acadrust::entities::text::TextHorizontalAlignment::Right => "right",
    };
    let v_align = match text.vertical_alignment {
        acadrust::entities::text::TextVerticalAlignment::Top => "top",
        acadrust::entities::text::TextVerticalAlignment::Middle => "middle",
        _ => "bottom", // Baseline | Bottom
    };
    let mut props = base_props(&color, &text.common, doc);
    props.insert("text".into(), json!(display_text));
    props.insert("fontSize".into(), json!(text.height));
    props.insert("rotation".into(), json!(rotation_deg));
    props.insert("textAlign".into(), json!(h_align));
    props.insert("textBaseline".into(), json!(v_align));
    make_feature_with_code(
        "Point",
        pt(wcs),
        Value::Object(props),
        text.common.handle.value(),
    )
}

/// Resolve the color of an MText span from its inline formatting codes.
///
/// Priority:
/// 1. `color_rgb` (from `\c<BGR>;` true-color code) — highest priority
/// 2. `color = MTextColor::Index(n)` (from `\C<n>;` ACI code) — look up ACI table
/// 3. `color = MTextColor::None` (from `\C256;` or `\C0;`) — ByLayer, resolve to
///    `layer_color` (the layer's own color, independent of the entity color)
/// 4. No color property set — fall back to `entity_color` (the entity-level color)
fn resolve_mtext_span_color(
    span: &acadrust::entities::mtext_format::MTextSpan,
    entity_color: &str,
    layer_color: &str,
) -> String {
    // True-color RGB override (from \c<BGR>;)
    if let Some((r, g, b)) = span.properties.color_rgb {
        return format!("{},{},{}", r, g, b);
    }
    // ACI index color (from \C<n>;)
    if let Some(ref c) = span.properties.color {
        use acadrust::entities::mtext_format::MTextColor;
        match c {
            MTextColor::Index(idx) => {
                if let Some((r, g, b)) = acadrust::types::aci_to_rgb(*idx as u8) {
                    return format!("{},{},{}", r, g, b);
                }
            }
            MTextColor::TrueColor(v) => {
                let r = ((v >> 16) & 0xFF) as u8;
                let g = ((v >> 8) & 0xFF) as u8;
                let b = (v & 0xFF) as u8;
                return format!("{},{},{}", r, g, b);
            }
            MTextColor::None => {
                // \C0 or \C256 = ByLayer: resolve to layer color
                return layer_color.to_string();
            }
        }
    }
    // No color property at all — use entity color
    entity_color.to_string()
}

fn mtext_to_features(mtext: &MText, doc: &CadDocument) -> Vec<Value> {
    let color = color_to_rgb_string(mtext.common.color, &mtext.common.layer, doc);
    // Layer color: used by \C256 / \C0 (ByLayer) inline MText color resets,
    // which should resolve to the layer's own color, not the entity's color.
    let layer_color = color_to_rgb_string(Color::ByLayer, &mtext.common.layer, doc);
    let wcs = ocs_to_wcs(mtext.normal, mtext.insertion_point);
    let rotation_deg = calc_text_rotation(mtext.rotation, mtext.normal);
    let mtext_doc = acadrust::entities::mtext_format::parse_mtext(&mtext.value, true);
    let display_text = mtext_to_display_text(&mtext_doc);
    let alignment = mtext.attachment_point as i32;
    // Map attachment_point (1-9) to OpenLayers textAlign / textBaseline
    let (text_align, text_baseline) = match mtext.attachment_point {
        AttachmentPoint::TopLeft => ("left", "top"),
        AttachmentPoint::TopCenter => ("center", "top"),
        AttachmentPoint::TopRight => ("right", "top"),
        AttachmentPoint::MiddleLeft => ("left", "middle"),
        AttachmentPoint::MiddleCenter => ("center", "middle"),
        AttachmentPoint::MiddleRight => ("right", "middle"),
        AttachmentPoint::BottomLeft => ("left", "bottom"),
        AttachmentPoint::BottomCenter => ("center", "bottom"),
        AttachmentPoint::BottomRight => ("right", "bottom"),
    };

    let mut base = base_props(&color, &mtext.common, doc);
    base.insert("text".into(), json!(display_text));
    base.insert("fontSize".into(), json!(mtext.height));
    base.insert("rotation".into(), json!(rotation_deg));
    base.insert("align".into(), json!(alignment));
    base.insert("textAlign".into(), json!(text_align));
    base.insert("textBaseline".into(), json!(text_baseline));
    base.insert("rectWidth".into(), json!(mtext.rectangle_width));
    if let Some(rh) = mtext.rectangle_height {
        base.insert("rectHeight".into(), json!(rh));
    }
    base.insert("lineSpacingFactor".into(), json!(mtext.line_spacing_factor));
    base.insert(
        "lineSpacingStyle".into(),
        json!(mtext.line_spacing_style as i32),
    );

    // Resolve inline MText color overrides (\C / \c codes) per span.
    // Collect all distinct span colors; if any span overrides the entity color,
    // output the first override as "color" and all per-span colors as "spanColors".
    let span_colors: Vec<String> = mtext_doc
        .paragraphs
        .iter()
        .flat_map(|p| p.spans.iter())
        .map(|s| resolve_mtext_span_color(s, &color, &layer_color))
        .collect();

    let has_override = span_colors.iter().any(|c| c != &color);
    if has_override {
        // The first non-default span color becomes the primary color
        if let Some(first_override) = span_colors.iter().find(|c| *c != &color) {
            base.insert("color".into(), json!(first_override.clone()));
        }
        // Always output the full per-span color array so the frontend can
        // render multi-color MText correctly.
        base.insert("spanColors".into(), json!(span_colors));
    }

    // Split multi-line MText into per-line Point features with computed positions
    // let lines: Vec<&str> = display_text.lines().collect();
    // if lines.len() <= 1 {
    return vec![make_feature_with_code(
        "Point",
        pt(wcs),
        Value::Object(base),
        mtext.common.handle.value(),
    )];
    // }

    // let line_height = mtext.height * mtext.line_spacing_factor * 1.2;
    // let total_height = line_height * (lines.len() as f64 - 1.0);
    // // Y offset for first line relative to insertion_point (Y-down in screen space)
    // let first_line_y_offset = match mtext.attachment_point {
    //     AttachmentPoint::TopLeft | AttachmentPoint::TopCenter | AttachmentPoint::TopRight => 0.0,
    //     AttachmentPoint::MiddleLeft
    //     | AttachmentPoint::MiddleCenter
    //     | AttachmentPoint::MiddleRight => -total_height / 2.0,
    //     AttachmentPoint::BottomLeft
    //     | AttachmentPoint::BottomCenter
    //     | AttachmentPoint::BottomRight => -total_height,
    // };

    // let features: Vec<Value> = lines
    //     .iter()
    //     .enumerate()
    //     .map(|(i, line)| {
    //         let y_off = first_line_y_offset + i as f64 * line_height;
    //         let pt_coord = Vector3::new(wcs.x, wcs.y - y_off, wcs.z);
    //         let mut props = base.clone();
    //         props.insert("text".into(), json!(line));
    //         props.insert("lineIndex".into(), json!(i));
    //         props.insert("lineCount".into(), json!(lines.len()));
    //         make_feature_with_code(
    //             "Point",
    //             pt(pt_coord),
    //             Value::Object(props),
    //             mtext.common.handle.value(),
    //         )
    //     })
    //     .collect();
    // features
}

/// Convert MTextDocument to display text, replacing subscript/superscript spans
/// with Unicode subscript/superscript characters so OpenLayers can render them.
fn mtext_to_display_text(mtext_doc: &acadrust::entities::mtext_format::MTextDocument) -> String {
    mtext_doc
        .paragraphs
        .iter()
        .map(|para| {
            para.spans
                .iter()
                .map(|span| {
                    if let Some(ref stack) = span.stacking {
                        return stacking_to_display_text(stack);
                    }
                    // Also check if the span itself has subscript-like height/alignment
                    let is_sub = is_subscript_span(&span.properties);
                    let is_sup = is_superscript_span(&span.properties);
                    if is_sub {
                        span.text.chars().map(to_unicode_subscript).collect()
                    } else if is_sup {
                        span.text.chars().map(to_unicode_superscript).collect()
                    } else {
                        span.text.clone()
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert stacking data to display text with Unicode subscript/superscript.
/// In CAD MText limit stacking (\S with ^):
///   numerator → superscript position (above baseline)
///   denominator → subscript position (below baseline)
///   \S^text; → denominator only → subscript (e.g. J₂)
///   \Stext^; → numerator only → superscript
///   \Snum^den; → sup+sub (limits like ⁺⁰·⁵₋₀.₃)
fn stacking_to_display_text(stack: &acadrust::entities::mtext_format::StackingData) -> String {
    use acadrust::entities::mtext_format::StackingType;
    match stack.stacking_type {
        StackingType::Limit => {
            let num_sup: String = stack
                .numerator
                .chars()
                .map(to_unicode_superscript)
                .collect();
            let den_sub: String = stack
                .denominator
                .chars()
                .map(to_unicode_subscript)
                .collect();
            format!("{}{}", num_sup, den_sub)
        }
        StackingType::Horizontal | StackingType::Diagonal => {
            format!("{}/{}", stack.numerator, stack.denominator)
        }
    }
}

fn is_subscript_span(props: &acadrust::entities::mtext_format::SpanProperties) -> bool {
    use acadrust::entities::mtext_format::*;
    let has_small_height = match props.height {
        Some(MTextScalar::Factor(v)) => v < 0.9,
        Some(MTextScalar::Absolute(_)) => false, // can't tell without base height
        None => false,
    };
    let is_baseline = matches!(props.line_align, None | Some(MTextLineAlignment::Bottom));
    has_small_height && is_baseline
}

fn is_superscript_span(props: &acadrust::entities::mtext_format::SpanProperties) -> bool {
    use acadrust::entities::mtext_format::*;
    let has_small_height = match props.height {
        Some(MTextScalar::Factor(v)) => v < 0.9,
        Some(MTextScalar::Absolute(_)) => false,
        None => false,
    };
    has_small_height && matches!(props.line_align, Some(MTextLineAlignment::Top))
}

fn to_unicode_subscript(ch: char) -> char {
    match ch {
        '0' => '\u{2080}',
        '1' => '\u{2081}',
        '2' => '\u{2082}',
        '3' => '\u{2083}',
        '4' => '\u{2084}',
        '5' => '\u{2085}',
        '6' => '\u{2086}',
        '7' => '\u{2087}',
        '8' => '\u{2088}',
        '9' => '\u{2089}',
        '+' => '\u{208A}',
        '-' | '\u{2212}' => '\u{208B}',
        '=' => '\u{208C}',
        '(' => '\u{208D}',
        ')' => '\u{208E}',
        'a' => '\u{2090}',
        'e' => '\u{2091}',
        'h' => '\u{2095}',
        'i' => '\u{1D62}',
        'j' => '\u{2C7C}',
        'k' => '\u{2096}',
        'l' => '\u{2097}',
        'm' => '\u{2098}',
        'n' => '\u{2099}',
        'o' => '\u{2092}',
        'p' => '\u{209A}',
        'r' => '\u{1D63}',
        's' => '\u{209B}',
        't' => '\u{209C}',
        'u' => '\u{1D64}',
        'v' => '\u{1D65}',
        'x' => '\u{2093}',
        _ => ch,
    }
}

fn to_unicode_superscript(ch: char) -> char {
    match ch {
        '0' => '\u{2070}',
        '1' => '\u{00B9}',
        '2' => '\u{00B2}',
        '3' => '\u{00B3}',
        '4' => '\u{2074}',
        '5' => '\u{2075}',
        '6' => '\u{2076}',
        '7' => '\u{2077}',
        '8' => '\u{2078}',
        '9' => '\u{2079}',
        '+' => '\u{207A}',
        '-' | '\u{2212}' => '\u{207B}',
        '=' => '\u{207C}',
        '(' => '\u{207D}',
        ')' => '\u{207E}',
        'n' => '\u{207F}',
        'i' => '\u{2071}',
        _ => ch,
    }
}

fn attrib_to_feature(attrib: &AttributeEntity, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(attrib.common.color, &attrib.common.layer, doc);
    // Always use insertion_point (left/baseline anchor) for consistent rendering
    let wcs = ocs_to_wcs(attrib.normal, attrib.insertion_point);
    let rotation_deg = calc_text_rotation(attrib.rotation, attrib.normal);
    let h_align = match attrib.horizontal_alignment {
        acadrust::entities::attribute_definition::HorizontalAlignment::Left
        | acadrust::entities::attribute_definition::HorizontalAlignment::Aligned
        | acadrust::entities::attribute_definition::HorizontalAlignment::Fit => "left",
        acadrust::entities::attribute_definition::HorizontalAlignment::Center
        | acadrust::entities::attribute_definition::HorizontalAlignment::Middle => "center",
        acadrust::entities::attribute_definition::HorizontalAlignment::Right => "right",
    };
    let v_align = match attrib.vertical_alignment {
        acadrust::entities::attribute_definition::VerticalAlignment::Top => "top",
        acadrust::entities::attribute_definition::VerticalAlignment::Middle => "middle",
        _ => "bottom", // Baseline | Bottom
    };
    let mut props = base_props(&color, &attrib.common, doc);
    props.insert("text".into(), json!(attrib.value));
    props.insert("fontSize".into(), json!(attrib.height));
    props.insert("rotation".into(), json!(rotation_deg));
    props.insert("textAlign".into(), json!(h_align));
    props.insert("textBaseline".into(), json!(v_align));
    make_feature_with_code(
        "Point",
        pt(wcs),
        Value::Object(props),
        attrib.common.handle.value(),
    )
}

fn lwpolyline_to_features(pl: &LwPolyline, doc: &CadDocument) -> Vec<Value> {
    let color = color_to_rgb_string(pl.common.color, &pl.common.layer, doc);
    let mut features = Vec::new();
    let verts = &pl.vertices;
    let n = verts.len();
    if n == 0 {
        features.push(make_feature(
            "LineString",
            Value::Array(vec![]),
            Value::Object(base_props(&color, &pl.common, doc)),
        ));
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
    if pl.is_closed && coords.len() > 1 {
        coords.push(coords[0].clone());
    }

    let mut props = base_props(&color, &pl.common, doc);
    let has_widths = verts
        .iter()
        .any(|v| v.start_width != 0.0 || v.end_width != 0.0);
    if has_widths {
        let widths: Vec<Value> = verts
            .iter()
            .map(|v| json!([v.start_width, v.end_width]))
            .collect();
        props.insert("widths".into(), Value::Array(widths));
    }
    if pl.constant_width != 0.0 {
        props.insert("constantWidth".into(), json!(pl.constant_width));
    }
    features.push(make_feature_with_code(
        "LineString",
        Value::Array(coords),
        Value::Object(props),
        pl.common.handle.value(),
    ));

    // 箭头段 → 闭合填充 Polygon
    for i in 0..n.saturating_sub(1) {
        let v0 = &verts[i];
        let v1 = &verts[i + 1];
        let sw = v0.start_width;
        let ew = v0.end_width;
        if (sw - ew).abs() < 1e-6 || (sw == 0.0 && ew == 0.0) {
            continue;
        }
        let p0 = v0.location;
        let p1 = v1.location;
        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len < 1e-10 {
            continue;
        }
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
        let corners_wcs: Vec<Vector3> = corners_2d
            .iter()
            .map(|c| basis * Vector3::new(c.x, c.y, pl.elevation))
            .collect();
        let mut ring: Vec<Value> = corners_wcs.iter().map(|c| pt(*c)).collect();
        ring.push(pt(corners_wcs[0]));
        let mut arrow_props = base_props(&color, &pl.common, doc);
        arrow_props.insert("arrow".into(), json!(true));
        arrow_props.insert("segment".into(), json!(i));
        arrow_props.insert("startWidth".into(), json!(sw));
        arrow_props.insert("endWidth".into(), json!(ew));
        features.push(make_feature_with_code(
            "Polygon",
            json!([ring]),
            Value::Object(arrow_props),
            pl.common.handle.value(),
        ));
    }
    features
}

fn polyline3d_to_feature(pl: &Polyline, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(pl.common.color, &pl.common.layer, doc);
    let coords: Vec<Value> = pl.vertices.iter().map(|v| pt(v.location)).collect();
    make_feature_with_code(
        "LineString",
        Value::Array(coords),
        Value::Object(base_props(&color, &pl.common, doc)),
        pl.common.handle.value(),
    )
}

fn polyline3d_new_to_feature(pl: &Polyline3D, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(pl.common.color, &pl.common.layer, doc);
    let coords: Vec<Value> = pl.vertices.iter().map(|v| pt(v.position)).collect();
    make_feature_with_code(
        "LineString",
        Value::Array(coords),
        Value::Object(base_props(&color, &pl.common, doc)),
        pl.common.handle.value(),
    )
}

fn polyline2d_to_feature(pl: &Polyline2D, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(pl.common.color, &pl.common.layer, doc);
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
            let arc_pts = tessellate_bulge(
                Vector2::new(start.x, start.y),
                Vector2::new(end.x, end.y),
                bulge,
            );
            for p in &arc_pts {
                let wcs_pt = basis * Vector3::new(p.x, p.y, pl.elevation);
                coords.push(pt(wcs_pt));
            }
        } else {
            let wcs_pt = basis * Vector3::new(start.x, start.y, pl.elevation);
            coords.push(pt(wcs_pt));
        }
    }
    if pl.is_closed() && coords.len() > 1 {
        coords.push(coords[0].clone());
    }
    make_feature_with_code(
        "LineString",
        Value::Array(coords),
        Value::Object(base_props(&color, &pl.common, doc)),
        pl.common.handle.value(),
    )
}

fn ellipse_to_feature(ellipse: &Ellipse, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(ellipse.common.color, &ellipse.common.layer, doc);
    let center = ellipse.center;
    let major = ellipse.major_axis;
    let major_len = major.length();
    let minor_len = major_len * ellipse.minor_axis_ratio;
    let u = major * (1.0 / major_len.max(1e-12));
    let v = ellipse.normal.cross(&u);
    let start_param = ellipse.start_parameter;
    let mut end_param = ellipse.end_parameter;
    if end_param <= start_param {
        end_param += 2.0 * std::f64::consts::PI;
    }
    let delta = 0.05;
    let mut coords = Vec::new();
    let mut t = start_param;
    while t <= end_param + delta {
        if t > end_param {
            t = end_param;
        }
        let p = center + u * (major_len * t.cos()) + v * (minor_len * t.sin());
        coords.push(pt(p));
        if (t - end_param).abs() < 1e-12 {
            break;
        }
        t += delta;
    }
    make_feature_with_code(
        "LineString",
        Value::Array(coords),
        Value::Object(base_props(&color, &ellipse.common, doc)),
        ellipse.common.handle.value(),
    )
}

fn spline_to_feature(spline: &Spline, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(spline.common.color, &spline.common.layer, doc);
    let pts = evaluate_nurbs(spline);
    let coords: Vec<Value> = pts.iter().map(|p| pt(*p)).collect();
    make_feature_with_code(
        "LineString",
        Value::Array(coords),
        Value::Object(base_props(&color, &spline.common, doc)),
        spline.common.handle.value(),
    )
}

fn evaluate_nurbs(spline: &Spline) -> Vec<Vector3> {
    let cps = &spline.control_points;
    let knots = &spline.knots;
    let weights = &spline.weights;
    let degree = spline.degree as usize;
    if cps.is_empty() || knots.is_empty() || degree == 0 {
        return cps.clone();
    }
    let n = cps.len();
    let use_weights = weights.len() == n;
    let t_min = if degree < knots.len() {
        knots[degree]
    } else {
        0.0
    };
    let t_max = if n < knots.len() {
        knots[n]
    } else {
        knots.last().copied().unwrap_or(1.0)
    };
    let num_samples = ((n * 10).max(20)).min(1000);
    let step = (t_max - t_min) / num_samples as f64;
    let mut result = Vec::with_capacity(num_samples + 1);
    for i in 0..=num_samples {
        let t = (t_min + i as f64 * step).min(t_max);
        let mut span = degree;
        for k in degree..knots.len().saturating_sub(1) {
            if t >= knots[k] && t < knots[k + 1] {
                span = k;
                break;
            }
            if k == knots.len() - 2 {
                span = k;
            }
        }
        let mut d: Vec<Vector3> = Vec::with_capacity(degree + 1);
        let mut w: Vec<f64> = Vec::with_capacity(degree + 1);
        for j in 0..=degree {
            let idx = if span >= degree && span - degree + j < n {
                span - degree + j
            } else {
                j.min(n - 1)
            };
            d.push(cps[idx]);
            w.push(if use_weights { weights[idx] } else { 1.0 });
        }
        for r in 1..=degree {
            for j in (r..=degree).rev() {
                let idx = span - degree + j;
                let left = if idx > 0 && idx <= knots.len() - 1 {
                    knots[idx]
                } else {
                    t
                };
                let right = if idx + degree - r + 1 < knots.len() {
                    knots[idx + degree - r + 1]
                } else {
                    t
                };
                let denom = right - left;
                let alpha = if denom.abs() < 1e-14 {
                    0.0
                } else {
                    (t - left) / denom
                };
                let prev = j - 1;
                d[j] = d[prev] * (1.0 - alpha) + d[j] * alpha;
                w[j] = w[prev] * (1.0 - alpha) + w[j] * alpha;
            }
        }
        let wf = w[degree];
        if wf.abs() > 1e-14 {
            result.push(d[degree] * (1.0 / wf));
        } else {
            result.push(d[degree]);
        }
    }
    result
}

fn hatch_to_feature(hatch: &Hatch, doc: &CadDocument) -> Value {
    let color = color_to_rgb_string(hatch.common.color, &hatch.common.layer, doc);
    let basis = Matrix3::arbitrary_axis(hatch.normal);

    // Collect rings together with their external flag
    let mut ring_entries: Vec<(Vec<Value>, bool)> = Vec::new();
    for path in &hatch.paths {
        let mut ring: Vec<Value> = Vec::new();
        for edge in &path.edges {
            match edge {
                BoundaryEdge::Line(le) => {
                    let s = basis * Vector3::new(le.start.x, le.start.y, 0.0);
                    let e = basis * Vector3::new(le.end.x, le.end.y, 0.0);
                    if ring.is_empty() {
                        ring.push(pt(s));
                    }
                    ring.push(pt(e));
                }
                BoundaryEdge::CircularArc(ae) => {
                    let c = ae.center;
                    let r = ae.radius;
                    let mut sa = ae.start_angle;
                    let mut ea = ae.end_angle;
                    if ea <= sa {
                        ea += 2.0 * std::f64::consts::PI;
                    }
                    if !ae.counter_clockwise {
                        std::mem::swap(&mut sa, &mut ea);
                        ea += 2.0 * std::f64::consts::PI;
                    }
                    let sweep = ea - sa;
                    let seg = (sweep / SMALLEST_ANGLE).ceil().max(1.0) as usize;
                    let step = sweep / seg as f64;
                    if ring.is_empty() {
                        ring.push(pt(basis
                            * (Vector3::new(c.x, c.y, 0.0)
                                + Vector3::new(r * sa.cos(), r * sa.sin(), 0.0))));
                    }
                    for i in 1..=seg {
                        let a = sa + i as f64 * step;
                        ring.push(pt(basis
                            * (Vector3::new(c.x, c.y, 0.0)
                                + Vector3::new(r * a.cos(), r * a.sin(), 0.0))));
                    }
                }
                BoundaryEdge::EllipticArc(ee) => {
                    let c = ee.center;
                    let me = ee.major_axis_endpoint;
                    let ml = me.length();
                    let ml2 = ml * ee.minor_axis_ratio;
                    let u = Vector3::new(me.x, me.y, 0.0) * (1.0 / ml.max(1e-12));
                    let v = Vector3::new(-u.y, u.x, 0.0);
                    let sa = ee.start_angle;
                    let mut ea = ee.end_angle;
                    if ea <= sa {
                        ea += 2.0 * std::f64::consts::PI;
                    }
                    let sweep = ea - sa;
                    let seg = (sweep / 0.05).ceil().max(1.0) as usize;
                    let step = sweep / seg as f64;
                    if ring.is_empty() {
                        ring.push(pt(basis
                            * (Vector3::new(c.x, c.y, 0.0)
                                + u * (ml * sa.cos())
                                + v * (ml2 * sa.sin()))));
                    }
                    for i in 1..=seg {
                        let t = sa + i as f64 * step;
                        ring.push(pt(basis
                            * (Vector3::new(c.x, c.y, 0.0)
                                + u * (ml * t.cos())
                                + v * (ml2 * t.sin()))));
                    }
                }
                BoundaryEdge::Polyline(pe) => {
                    let vs = &pe.vertices;
                    let nv = vs.len();
                    for i in 0..nv {
                        let v = &vs[i];
                        let bulge = v.z;
                        let s = Vector2::new(v.x, v.y);
                        if bulge.abs() > 1e-10 && i < nv - 1 {
                            let nx = &vs[(i + 1) % nv];
                            let e = Vector2::new(nx.x, nx.y);
                            for p in &tessellate_bulge(s, e, bulge) {
                                ring.push(pt(basis * Vector3::new(p.x, p.y, 0.0)));
                            }
                        } else {
                            ring.push(pt(basis * Vector3::new(s.x, s.y, 0.0)));
                        }
                    }
                    if pe.is_closed && ring.len() > 1 {
                        ring.push(ring[0].clone());
                    }
                }
                BoundaryEdge::Spline(se) => {
                    for cp in &se.control_points {
                        ring.push(pt(basis * Vector3::new(cp.x, cp.y, 0.0)));
                    }
                }
            }
        }
        if ring.len() > 1 && ring.first() != ring.last() {
            ring.push(ring[0].clone());
        }
        if !ring.is_empty() {
            ring_entries.push((ring, path.flags.is_external()));
        }
    }

    // Build GeoJSON geometry:
    //   - Single ring → Polygon: [[ring]]
    //   - One outer + holes  → Polygon: [[outer], [hole1], [hole2], ...]
    //   - Multiple outers    → MultiPolygon: [[[outer1], [hole]], [[outer2]], ...]
    let (geo_type, coords) = if ring_entries.is_empty() {
        ("Polygon", Value::Array(vec![]))
    } else {
        let outer_count = ring_entries.iter().filter(|(_, ext)| *ext).count();
        if outer_count <= 1 {
            // All rings form a single polygon (first = outer, rest = holes)
            let rings: Vec<Value> = ring_entries
                .into_iter()
                .map(|(r, _)| Value::Array(r))
                .collect();
            ("Polygon", Value::Array(rings))
        } else {
            // Multiple disconnected outer boundaries → MultiPolygon
            // Assign each hole to the preceding outer boundary.
            let mut polygons: Vec<Vec<Value>> = Vec::new();
            let mut current: Vec<Value> = Vec::new();
            for (ring, is_ext) in ring_entries {
                if is_ext {
                    if !current.is_empty() {
                        polygons.push(current);
                    }
                    current = vec![Value::Array(ring)];
                } else {
                    current.push(Value::Array(ring));
                }
            }
            if !current.is_empty() {
                polygons.push(current);
            }
            let mp: Vec<Value> = polygons.into_iter().map(Value::Array).collect();
            ("MultiPolygon", Value::Array(mp))
        }
    };

    let mut hatch_props = base_props(&color, &hatch.common, doc);
    hatch_props.insert("fill".into(), json!(true));
    hatch_props.insert("entityType".into(), json!("hatch"));

    hatch_props.insert("isSolid".into(), json!(hatch.is_solid));
    hatch_props.insert("patternName".into(), json!(hatch.pattern.name));
    hatch_props.insert("patternScale".into(), json!(hatch.pattern_scale));
    hatch_props.insert("patternAngle".into(), json!(hatch.pattern_angle.to_degrees()));

    // Export pattern lines for non-solid hatches (for frontend canvas rendering)
    if !hatch.is_solid && !hatch.pattern.lines.is_empty() {
        let pat_lines: Vec<Value> = hatch.pattern.lines.iter().map(|pl| {
            json!({
                "angle": pl.angle.to_degrees(),
                "basePoint": [pl.base_point.x, pl.base_point.y],
                "offset": [pl.offset.x, pl.offset.y],
                "dashes": pl.dash_lengths,
            })
        }).collect();
        hatch_props.insert("patternLines".into(), Value::Array(pat_lines));
    }

    make_feature_with_code(
        geo_type,
        coords,
        Value::Object(hatch_props),
        hatch.common.handle.value(),
    )
}

fn solid_to_feature(s: &Solid, doc: &CadDocument) -> Value {
    let c = color_to_rgb_string(s.common.color, &s.common.layer, doc);
    let b = Matrix3::arbitrary_axis(s.normal);
    let p1 = b * s.first_corner;
    let p2 = b * s.second_corner;
    let p3 = b * s.third_corner;
    let p4 = b * s.fourth_corner;
    let mut r = vec![pt(p1), pt(p2), pt(p3)];
    if !s.is_triangle() {
        r.push(pt(p4));
    }
    r.push(r[0].clone());
    let mut props = base_props(&c, &s.common, doc);
    props.insert("fill".into(), json!(true));
    props.insert("entityType".into(), json!("solid"));
    make_feature_with_code(
        "Polygon",
        json!([r]),
        Value::Object(props),
        s.common.handle.value(),
    )
}

fn face3d_to_feature(f: &Face3D, doc: &CadDocument) -> Value {
    let c = color_to_rgb_string(f.common.color, &f.common.layer, doc);
    let mut r = vec![pt(f.first_corner), pt(f.second_corner), pt(f.third_corner)];
    if !f.is_triangle() {
        r.push(pt(f.fourth_corner));
    }
    r.push(r[0].clone());
    let mut props = base_props(&c, &f.common, doc);
    props.insert("fill".into(), json!(true));
    props.insert("entityType".into(), json!("face3d"));
    make_feature_with_code(
        "Polygon",
        json!([r]),
        Value::Object(props),
        f.common.handle.value(),
    )
}
// fn dimension_to_features(dim: &Dimension, doc: &CadDocument) -> Vec<Value> {
//     let base = dim.base();
//     let color = color_to_rgb_string(base.common.color, &base.common.layer, doc);
//     let handle = base.common.handle.value();
//     let mut features = Vec::new();
//     // 参考线（LineString）：大部分类型产生一条，Arc 可能需要多条
//     let mut line_groups: Vec<Vec<Value>> = match dim {
//         Dimension::Linear(d) => vec![vec![
//             pt(d.first_point),
//             pt(d.definition_point),
//             pt(d.second_point),
//         ]],
//         Dimension::Aligned(d) => vec![vec![
//             pt(d.first_point),
//             pt(d.definition_point),
//             pt(d.second_point),
//         ]],
//         Dimension::Radius(d) => vec![vec![pt(d.angle_vertex), pt(d.definition_point)]],
//         Dimension::Diameter(d) => vec![vec![pt(d.angle_vertex), pt(d.definition_point)]],
//         Dimension::Angular2Ln(d) => vec![vec![
//             pt(d.first_point),
//             pt(d.angle_vertex),
//             pt(d.second_point),
//             pt(d.dimension_arc),
//         ]],
//         Dimension::Angular3Pt(d) => vec![vec![
//             pt(d.first_point),
//             pt(d.angle_vertex),
//             pt(d.second_point),
//         ]],
//         Dimension::Ordinate(d) => vec![vec![
//             pt(d.definition_point),
//             pt(d.feature_location),
//             pt(d.leader_endpoint),
//         ]],
//         Dimension::Arc(d) => {
//             // 圆心→弧起点、圆心→弧终点 + 可选引线
//             let mut lines = vec![
//                 vec![pt(d.center_point), pt(d.first_extension_point)],
//                 vec![pt(d.center_point), pt(d.second_extension_point)],
//             ];
//             if d.has_leader {
//                 lines.push(vec![pt(d.first_leader_point), pt(d.second_leader_point)]);
//             }
//             lines
//         }
//         Dimension::LargeRadial(d) => {
//             // 折弯标注：definition_point → jog_point → chord_point
//             vec![vec![pt(d.definition_point), pt(d.jog_point), pt(d.chord_point)]]
//         }
//     };
//     for lc in line_groups.drain(..) {
//         if lc.len() >= 2 {
//             features.push(make_feature_with_code(
//                 "LineString",
//                 Value::Array(lc),
//                 Value::Object(base_props(&color, &base.common, doc)),
//                 handle,
//             ));
//         }
//     }
//     let dt = if !base.text.is_empty() {
//         base.text.clone()
//     } else if let Some(ref ut) = base.user_text {
//         ut.clone()
//     } else {
//         format!("{:.2}", base.actual_measurement)
//     };
//     let ds = match dim {
//         Dimension::Linear(_) => "linear",
//         Dimension::Aligned(_) => "aligned",
//         Dimension::Radius(_) => "radius",
//         Dimension::Diameter(_) => "diameter",
//         Dimension::Angular2Ln(_) => "angular",
//         Dimension::Angular3Pt(_) => "angular3pt",
//         Dimension::Ordinate(_) => "ordinate",
//         Dimension::Arc(_) => "arc",
//         Dimension::LargeRadial(_) => "large_radial",
//     };
//     let rot = calc_text_rotation(base.text_rotation, base.normal);
//     let mut props = base_props(&color, &base.common, doc);
//     props.insert("text".into(), json!(dt));
//     props.insert("fontSize".into(), json!(0.0));
//     props.insert("rotation".into(), json!(rot));
//     props.insert("measurement".into(), json!(base.actual_measurement));
//     props.insert("dimensionType".into(), json!(ds));
//     features.push(make_feature_with_code(
//         "Point",
//         pt(base.text_middle_point),
//         Value::Object(props),
//         handle,
//     ));
//     features
// }

// ═══════════════════════════════════════════════════════════════
//  Dimension 辅助函数
// ═══════════════════════════════════════════════════════════════

/// 查找 DimStyle（大小写不敏感）
fn resolve_dim_style<'a>(style_name: &str, doc: &'a CadDocument) -> Option<&'a DimStyle> {
    doc.dim_styles.get(style_name)
}

/// 获取 DimStyle 文字高度（dimtxt × dimscale），默认 2.5
fn dim_text_height(style_name: &str, doc: &CadDocument) -> f64 {
    resolve_dim_style(style_name, doc)
        .map(|s| s.dimtxt * s.dimscale)
        .unwrap_or(2.5)
}

/// 获取 DimStyle 箭头大小（dimasz × dimscale），默认 2.5
fn dim_arrow_size(style_name: &str, doc: &CadDocument) -> f64 {
    resolve_dim_style(style_name, doc)
        .map(|s| s.dimasz * s.dimscale)
        .unwrap_or(2.5)
}

/// 获取 DimStyle 文字颜色字符串，ByBlock(0) 时回退到实体颜色
fn dim_text_color_str(style_name: &str, parent_color: Color, parent_layer: &str, doc: &CadDocument) -> String {
    let aci = resolve_dim_style(style_name, doc)
        .map(|s| s.dimclrt)
        .unwrap_or(0);
    if aci == 0 {
        color_to_rgb_string(parent_color, parent_layer, doc)
    } else {
        color_to_rgb_string(Color::Index(aci as u8), parent_layer, doc)
    }
}

/// 解析标注文字内容：优先 text > user_text > 格式化测量值
fn dimension_display_text(base: &DimensionBase) -> String {
    if !base.text.is_empty() {
        return base.text.clone();
    }
    if let Some(ref ut) = base.user_text {
        if !ut.is_empty() {
            return ut.clone();
        }
    }
    let m = base.actual_measurement;
    if m.abs() < 1e-9 {
        "0".to_string()
    } else if (m - m.round()).abs() < 1e-9 {
        format!("{}", m.round() as i64)
    } else {
        format!("{:.2}", m)
    }
}

/// 标注类型字符串
fn dimension_type_str(dim: &Dimension) -> &'static str {
    match dim {
        Dimension::Linear(_) => "linear",
        Dimension::Aligned(_) => "aligned",
        Dimension::Radius(_) => "radius",
        Dimension::Diameter(_) => "diameter",
        Dimension::Angular2Ln(_) => "angular",
        Dimension::Angular3Pt(_) => "angular3pt",
        Dimension::Ordinate(_) => "ordinate",
        Dimension::Arc(_) => "arc",
        Dimension::LargeRadial(_) => "large_radial",
    }
}

/// 生成带 dimensionPart 标记的 LineString Feature
fn dim_line_feature(
    coords: Vec<Value>,
    part: &str,
    color: &str,
    common: &EntityCommon,
    doc: &CadDocument,
) -> Value {
    let mut props = base_props(color, common, doc);
    props.insert("dimensionPart".into(), json!(part));
    make_feature_with_code("LineString", Value::Array(coords), Value::Object(props), common.handle.value())
}

/// 生成带 dimensionPart 标记的 Polygon（箭头）Feature
fn dim_arrow_feature(
    triangle: Vec<Value>,
    part: &str,
    color: &str,
    common: &EntityCommon,
    doc: &CadDocument,
) -> Value {
    let mut props = base_props(color, common, doc);
    props.insert("dimensionPart".into(), json!(part));
    props.insert("fill".into(), json!(true));
    make_feature_with_code("Polygon", json!([triangle]), Value::Object(props), common.handle.value())
}

/// 生成等腰三角形箭头坐标（闭合：tip → p1 → p2 → tip）
/// dir: 从尾部指向尖端的向量
fn make_arrow_triangle(tip: Vector3, dir: Vector3, size: f64) -> Vec<Value> {
    let len = dir.length();
    if len < 1e-12 {
        return vec![pt(tip), pt(tip), pt(tip), pt(tip)];
    }
    let d = dir / len;
    let perp = Vector3::new(-d.y, d.x, 0.0);
    let half_w = size * 0.3;
    let tail = tip - d * size;
    let p1 = tail + perp * half_w;
    let p2 = tail - perp * half_w;
    vec![pt(tip), pt(p1), pt(p2), pt(tip)]
}

/// 离散化角度弧线（vertex 为圆心，radius 为半径，WCS XY 平面）
fn tessellate_angle_arc_pts(vertex: Vector3, start_angle: f64, end_angle: f64, radius: f64) -> Vec<Value> {
    let mut sweep = end_angle - start_angle;
    if sweep < 0.0 {
        sweep += 2.0 * std::f64::consts::PI;
    }
    if sweep > 2.0 * std::f64::consts::PI {
        sweep -= 2.0 * std::f64::consts::PI;
    }
    let segments = (sweep / SMALLEST_ANGLE).ceil().max(1.0) as usize;
    let step = sweep / segments as f64;
    let mut pts = Vec::with_capacity(segments + 1);
    for i in 0..=segments {
        let a = start_angle + i as f64 * step;
        pts.push(pt(Vector3::new(
            vertex.x + radius * a.cos(),
            vertex.y + radius * a.sin(),
            vertex.z,
        )));
    }
    pts
}

/// 展开 Dimension 匿名块，将块内实体转为 GeoJSON Features
/// ByBlock 颜色继承实体颜色，图层 "0" 继承实体图层
fn expand_dimension_block(
    block_name: &str,
    parent_color: Color,
    parent_layer: &str,
    doc: &CadDocument,
) -> Vec<Value> {
    if block_name.is_empty() {
        return Vec::new();
    }
    let br = match doc.block_records.get(block_name) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut features = Vec::new();
    for h in &br.entity_handles {
        let entity = match doc.get_entity(*h) {
            Some(e) => e,
            None => continue,
        };
        let mut ent = entity.clone();
        if ent.common().color == Color::ByBlock {
            ent.common_mut().color = parent_color;
        }
        if ent.common().layer == "0" {
            ent.common_mut().layer = parent_layer.to_string();
        }
        if let EntityType::Insert(ins) = &ent {
            // 展开箭头 Insert 块（如 _CLOSED、_OBLIQUE 等箭头块引用）
            let exploded = ins.explode_from_document(doc);
            for mut sub in exploded {
                if sub.common().color == Color::ByBlock {
                    sub.common_mut().color = parent_color;
                }
                if sub.common().layer == "0" {
                    sub.common_mut().layer = parent_layer.to_string();
                }
                if let Some(fs) = entity_to_features(&sub, doc) {
                    for mut f in fs {
                        if let Some(props) = f.get_mut("properties").and_then(|p| p.as_object_mut()) {
                            props.insert("dimensionPart".into(), json!("arrow"));
                        }
                        features.push(f);
                    }
                }
            }
        } else if let Some(fs) = entity_to_features(&ent, doc) {
            for mut f in fs {
                let geom_type = f.get("geometry")
                    .and_then(|g| g.get("type"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(props) = f.get_mut("properties").and_then(|p| p.as_object_mut()) {
                    let part = match geom_type.as_str() {
                        "LineString" => "dimensionLine",
                        "Point" => "dimensionText",
                        "Polygon" => "arrow",
                        _ => "dimensionPart",
                    };
                    props.insert("dimensionPart".into(), json!(part));
                }
                features.push(f);
            }
        }
    }
    features
}

// ═══════════════════════════════════════════════════════════════
//  Dimension 各类型 Fallback 几何生成
// ═══════════════════════════════════════════════════════════════

/// Linear 标注 Fallback 几何
fn gen_linear_dim_features(d: &DimensionLinear, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let arrow_sz = dim_arrow_size(&d.base.style_name, doc);
    let mut feats = Vec::new();
    // 简化：definition_point 作为延伸线终点
    let ext1_end = d.definition_point;
    let ext2_end = d.definition_point + (d.second_point - d.first_point);
    feats.push(dim_line_feature(vec![pt(d.first_point), pt(ext1_end)], "extension1", color, common, doc));
    feats.push(dim_line_feature(vec![pt(d.second_point), pt(ext2_end)], "extension2", color, common, doc));
    feats.push(dim_line_feature(vec![pt(ext1_end), pt(ext2_end)], "dimensionLine", color, common, doc));
    feats.push(dim_arrow_feature(make_arrow_triangle(ext1_end, ext2_end - ext1_end, arrow_sz), "arrow1", color, common, doc));
    feats.push(dim_arrow_feature(make_arrow_triangle(ext2_end, ext1_end - ext2_end, arrow_sz), "arrow2", color, common, doc));
    feats
}

/// Aligned 标注 Fallback 几何
fn gen_aligned_dim_features(d: &DimensionAligned, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let arrow_sz = dim_arrow_size(&d.base.style_name, doc);
    let mut feats = Vec::new();
    let line_dir = d.second_point - d.first_point;
    let len = line_dir.length();
    if len < 1e-12 { return feats; }
    let unit = line_dir / len;
    let perp = Vector3::new(-unit.y, unit.x, 0.0);
    let offset = (d.definition_point - d.first_point).dot(&perp);
    let ext1_end = d.first_point + perp * offset;
    let ext2_end = d.second_point + perp * offset;
    feats.push(dim_line_feature(vec![pt(d.first_point), pt(ext1_end)], "extension1", color, common, doc));
    feats.push(dim_line_feature(vec![pt(d.second_point), pt(ext2_end)], "extension2", color, common, doc));
    feats.push(dim_line_feature(vec![pt(ext1_end), pt(ext2_end)], "dimensionLine", color, common, doc));
    feats.push(dim_arrow_feature(make_arrow_triangle(ext1_end, ext2_end - ext1_end, arrow_sz), "arrow1", color, common, doc));
    feats.push(dim_arrow_feature(make_arrow_triangle(ext2_end, ext1_end - ext2_end, arrow_sz), "arrow2", color, common, doc));
    feats
}

/// Radius 标注 Fallback 几何
fn gen_radius_dim_features(d: &DimensionRadius, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let arrow_sz = dim_arrow_size(&d.base.style_name, doc);
    let mut feats = Vec::new();
    feats.push(dim_line_feature(vec![pt(d.angle_vertex), pt(d.definition_point)], "leader", color, common, doc));
    let dir = d.angle_vertex - d.definition_point;
    feats.push(dim_arrow_feature(make_arrow_triangle(d.definition_point, dir, arrow_sz), "arrow", color, common, doc));
    feats
}

/// Diameter 标注 Fallback 几何
fn gen_diameter_dim_features(d: &DimensionDiameter, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let arrow_sz = dim_arrow_size(&d.base.style_name, doc);
    let mut feats = Vec::new();
    feats.push(dim_line_feature(vec![pt(d.angle_vertex), pt(d.definition_point)], "diameterLine", color, common, doc));
    feats.push(dim_arrow_feature(make_arrow_triangle(d.angle_vertex, d.definition_point - d.angle_vertex, arrow_sz), "arrow1", color, common, doc));
    feats.push(dim_arrow_feature(make_arrow_triangle(d.definition_point, d.angle_vertex - d.definition_point, arrow_sz), "arrow2", color, common, doc));
    feats
}

/// Angular2Ln 标注 Fallback 几何
fn gen_angular2ln_dim_features(d: &DimensionAngular2Ln, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let mut feats = Vec::new();
    feats.push(dim_line_feature(vec![pt(d.angle_vertex), pt(d.first_point)], "side1", color, common, doc));
    feats.push(dim_line_feature(vec![pt(d.angle_vertex), pt(d.second_point)], "side2", color, common, doc));
    let v1 = d.first_point - d.angle_vertex;
    let v2 = d.second_point - d.angle_vertex;
    let r = d.dimension_arc.distance(&d.angle_vertex);
    if r > 1e-12 {
        let a1 = v1.y.atan2(v1.x);
        let a2 = v2.y.atan2(v2.x);
        let arc_pts = tessellate_angle_arc_pts(d.angle_vertex, a1, a2, r);
        if arc_pts.len() >= 2 {
            feats.push(dim_line_feature(arc_pts, "arc", color, common, doc));
        }
    }
    feats
}

/// Angular3Pt 标注 Fallback 几何
fn gen_angular3pt_dim_features(d: &DimensionAngular3Pt, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let mut feats = Vec::new();
    feats.push(dim_line_feature(vec![pt(d.angle_vertex), pt(d.first_point)], "side1", color, common, doc));
    feats.push(dim_line_feature(vec![pt(d.angle_vertex), pt(d.second_point)], "side2", color, common, doc));
    let v1 = d.first_point - d.angle_vertex;
    let v2 = d.second_point - d.angle_vertex;
    let r = d.definition_point.distance(&d.angle_vertex);
    if r > 1e-12 {
        let a1 = v1.y.atan2(v1.x);
        let a2 = v2.y.atan2(v2.x);
        let arc_pts = tessellate_angle_arc_pts(d.angle_vertex, a1, a2, r);
        if arc_pts.len() >= 2 {
            feats.push(dim_line_feature(arc_pts, "arc", color, common, doc));
        }
    }
    feats
}

/// Ordinate 标注 Fallback 几何
fn gen_ordinate_dim_features(d: &DimensionOrdinate, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    vec![dim_line_feature(vec![pt(d.feature_location), pt(d.leader_endpoint)], "leader", color, common, doc)]
}

/// Arc 弧长标注 Fallback 几何
fn gen_arc_dim_features(d: &DimensionArc, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let arrow_sz = dim_arrow_size(&d.base.style_name, doc);
    let mut feats = Vec::new();
    // 两条从圆心到弧端点的引线
    feats.push(dim_line_feature(
        vec![pt(d.center_point), pt(d.first_extension_point)], "extension1", color, common, doc));
    feats.push(dim_line_feature(
        vec![pt(d.center_point), pt(d.second_extension_point)], "extension2", color, common, doc));
    // 弧线（弧端点之间的圆弧）
    let r = d.center_point.distance(&d.first_extension_point);
    if r > 1e-12 {
        let a1 = (d.first_extension_point.y - d.center_point.y)
            .atan2(d.first_extension_point.x - d.center_point.x);
        let a2 = (d.second_extension_point.y - d.center_point.y)
            .atan2(d.second_extension_point.x - d.center_point.x);
        let arc_pts = tessellate_angle_arc_pts(d.center_point, a1, a2, r);
        if arc_pts.len() >= 2 {
            feats.push(dim_line_feature(arc_pts, "arc", color, common, doc));
        }
    }
    // 可选引线
    if d.has_leader {
        feats.push(dim_line_feature(
            vec![pt(d.first_leader_point), pt(d.second_leader_point)], "leader", color, common, doc));
        let dir = d.second_leader_point - d.first_leader_point;
        feats.push(dim_arrow_feature(
            make_arrow_triangle(d.second_leader_point, dir, arrow_sz), "arrow", color, common, doc));
    }
    feats
}

/// LargeRadial 折弯半径标注 Fallback 几何
fn gen_large_radial_dim_features(d: &DimensionLargeRadial, color: &str, doc: &CadDocument) -> Vec<Value> {
    let common = &d.base.common;
    let arrow_sz = dim_arrow_size(&d.base.style_name, doc);
    let mut feats = Vec::new();
    // 折弯引线: definition_point → jog_point → chord_point
    feats.push(dim_line_feature(
        vec![pt(d.definition_point), pt(d.jog_point), pt(d.chord_point)], "jogLeader", color, common, doc));
    // 箭头指向 definition_point（圆弧上的点）
    let dir = d.definition_point - d.jog_point;
    feats.push(dim_arrow_feature(
        make_arrow_triangle(d.definition_point, dir, arrow_sz), "arrow", color, common, doc));
    feats
}

/// 构建 Dimension 文字标注 Point Feature（始终添加）
fn build_dimension_text_feature(dim: &Dimension, doc: &CadDocument) -> Value {
    let base = dim.base();
    let handle = base.common.handle.value();
    let color = color_to_rgb_string(base.common.color, &base.common.layer, doc);
    let text = dimension_display_text(base);
    let font_size = dim_text_height(&base.style_name, doc);
    let text_color = dim_text_color_str(&base.style_name, base.common.color, &base.common.layer, doc);
    let rotation = calc_text_rotation(base.text_rotation, base.normal);
    let dt = dimension_type_str(dim);

    let mut props = base_props(&color, &base.common, doc);
    props.insert("text".into(), json!(text));
    props.insert("fontSize".into(), json!(font_size));
    props.insert("textColor".into(), json!(text_color));
    props.insert("rotation".into(), json!(rotation));
    props.insert("measurement".into(), json!(base.actual_measurement));
    props.insert("dimensionType".into(), json!(dt));
    props.insert("styleName".into(), json!(base.style_name));
    if let Dimension::Ordinate(d) = dim {
        props.insert("ordinateAxis".into(), json!(if d.is_ordinate_type_x { "X" } else { "Y" }));
    }
    make_feature_with_code("Point", pt(base.text_middle_point), Value::Object(props), handle)
}

fn dimension_to_features(dim: &Dimension, doc: &CadDocument) -> Vec<Value> {
    let base = dim.base();
    let color = color_to_rgb_string(base.common.color, &base.common.layer, doc);
    let mut features = Vec::new();
    let mut block_has_text = false;

    // 预计算尺寸标注语义属性（供块内文字增强使用）
    let font_size = dim_text_height(&base.style_name, doc);
    let text_color = dim_text_color_str(&base.style_name, base.common.color, &base.common.layer, doc);
    let dt = dimension_type_str(dim);

    // ① 尝试展开匿名块（AutoCAD 预渲染的精确几何）
    if !base.block_name.is_empty() {
        let mut block_feats = expand_dimension_block(
            &base.block_name, base.common.color, &base.common.layer, doc
        );
        if !block_feats.is_empty() {
            // 检查块内是否已包含文字 Point，并为文字 Feature 注入尺寸标注语义属性
            for f in &mut block_feats {
                let is_point = f.get("geometry")
                    .and_then(|g| g.get("type"))
                    .and_then(|t| t.as_str())
                    == Some("Point");
                if is_point {
                    block_has_text = true;
                    if let Some(props) = f.get_mut("properties").and_then(|p| p.as_object_mut()) {
                        props.insert("dimensionType".into(), json!(dt));
                        props.insert("measurement".into(), json!(base.actual_measurement));
                        props.insert("styleName".into(), json!(base.style_name));
                        // 仅当块内文字没有 fontSize 时注入
                        if !props.contains_key("fontSize") {
                            props.insert("fontSize".into(), json!(font_size));
                        }
                        if !props.contains_key("textColor") {
                            props.insert("textColor".into(), json!(text_color));
                        }
                        if let Dimension::Ordinate(d) = dim {
                            props.insert("ordinateAxis".into(), json!(if d.is_ordinate_type_x { "X" } else { "Y" }));
                        }
                    }
                }
            }
            features.extend(block_feats);
        }
    }

    // ② Fallback：从关键点生成完整几何
    if features.is_empty() {
        let geom_feats = match dim {
            Dimension::Linear(d)    => gen_linear_dim_features(d, &color, doc),
            Dimension::Aligned(d)   => gen_aligned_dim_features(d, &color, doc),
            Dimension::Radius(d)    => gen_radius_dim_features(d, &color, doc),
            Dimension::Diameter(d)  => gen_diameter_dim_features(d, &color, doc),
            Dimension::Angular2Ln(d)=> gen_angular2ln_dim_features(d, &color, doc),
            Dimension::Angular3Pt(d)=> gen_angular3pt_dim_features(d, &color, doc),
            Dimension::Ordinate(d)  => gen_ordinate_dim_features(d, &color, doc),
            Dimension::Arc(d)     => gen_arc_dim_features(d, &color, doc),
            Dimension::LargeRadial(d) => gen_large_radial_dim_features(d, &color, doc),
        };
        features.extend(geom_feats);
    }

    // ③ 仅在匿名块不包含文字时，添加语义 Point Feature（避免重复显示数字）
    if !block_has_text {
        features.push(build_dimension_text_feature(dim, doc));
    }

    features
}

fn helix_to_feature(h: &Helix, doc: &CadDocument) -> Value {
    spline_to_feature(&h.spline, doc)
}

fn has_empty_coords(f: &Value) -> bool {
    if let Some(g) = f.get("geometry") {
        if let Some(c) = g.get("coordinates") {
            return check_empty_coords(c);
        }
        return true;
    }
    true
}
fn check_empty_coords(v: &Value) -> bool {
    match v {
        Value::Array(a) => a.is_empty() || a.iter().any(check_empty_coords),
        _ => false,
    }
}

fn read_crs_from_doc(doc: &CadDocument, _q: bool) -> Value {
    let def = json!({"type":"name","properties":{"name":"urn:ogc:def:crs:EPSG::900913"}});
    for obj in doc.objects.values() {
        if let ObjectType::GeoData(gd) = obj {
            if gd.coordinate_system_definition.is_empty() {
                continue;
            }
            if let Some(e) = extract_epsg_from_wkt(&gd.coordinate_system_definition) {
                return json!({"type":"name","properties":{"name":format!("urn:ogc:def:crs:EPSG::{}",e)}});
            }
            if let Some(n) = extract_crs_from_xml(&gd.coordinate_system_definition) {
                return json!({"type":"name","properties":{"name":n}});
            }
        }
    }
    def
}
fn extract_epsg_from_wkt(wkt: &str) -> Option<String> {
    let w = wkt.to_uppercase();
    let p = "AUTHORITY[\"EPSG\",\"";
    if let Some(pos) = w.find(p) {
        let s = pos + p.len();
        if let Some(e) = wkt[s..].find('"') {
            return Some(wkt[s..s + e].to_string());
        }
    }
    None
}
fn extract_crs_from_xml(xml: &str) -> Option<String> {
    if let Some(c) = xtag(xml, "EPSG_CODE") {
        return Some(format!("urn:ogc:def:crs:EPSG::{}", c));
    }
    if let Some(n) = xtag(xml, "CS_NAME") {
        return Some(format!("urn:ogc:def:crs:EPSG::{}", n));
    }
    None
}
fn xtag(xml: &str, tag: &str) -> Option<String> {
    let o = format!("<{}>", tag);
    let c = format!("</{}>", tag);
    if let Some(s) = xml.find(&o) {
        let vs = s + o.len();
        if let Some(e) = xml[vs..].find(&c) {
            return Some(xml[vs..vs + e].trim().to_string());
        }
    }
    None
}

fn collect_positioning_baseline_features(
    doc: &CadDocument,
    layer_name: &str,
    exploded_by_layer: &std::collections::HashMap<String, Vec<EntityType>>,
) -> Vec<Value> {
    let mut features = Vec::new();
    struct TI {
        pos: Vector3,
        text: String,
    }
    struct LE {
        verts: Vec<Vector3>,
        feats: Vec<Value>,
    }
    let mut texts: Vec<TI> = Vec::new();
    let mut lines: Vec<LE> = Vec::new();
    // Keep original text entities for feature output (font size, color, etc.)
    let mut text_entities: Vec<EntityType> = Vec::new();

    // Helper closure: classify one entity into texts or lines
    let mut classify = |e: &EntityType| match e {
        EntityType::Text(t) => {
            let rp = t.alignment_point.unwrap_or(t.insertion_point);
            texts.push(TI {
                pos: ocs_to_wcs(t.normal, rp),
                text: acadrust::entities::mtext_format::parse_plain_text(&t.value).to_plain_text(),
            });
            text_entities.push(e.clone());
        }
        EntityType::MText(t) => {
            texts.push(TI {
                pos: ocs_to_wcs(t.normal, t.insertion_point),
                text: acadrust::entities::mtext_format::parse_mtext(&t.value, true).to_plain_text(),
            });
            text_entities.push(e.clone());
        }
        EntityType::Line(l) => {
            lines.push(LE {
                verts: vec![l.start, l.end],
                feats: vec![line_to_feature(l, doc)],
            });
        }
        EntityType::LwPolyline(p) => {
            let f = lwpolyline_to_features(p, doc);
            let b = Matrix3::arbitrary_axis(p.normal);
            lines.push(LE {
                verts: p
                    .vertices
                    .iter()
                    .map(|v| b * Vector3::new(v.location.x, v.location.y, p.elevation))
                    .collect(),
                feats: f,
            });
        }
        EntityType::Polyline2D(p) => {
            let f = polyline2d_to_feature(p, doc);
            let b = Matrix3::arbitrary_axis(p.normal);
            lines.push(LE {
                verts: p
                    .vertices
                    .iter()
                    .map(|v| b * Vector3::new(v.location.x, v.location.y, p.elevation))
                    .collect(),
                feats: vec![f],
            });
        }
        _ => {}
    };

    // Direct model-space entities on this layer
    for e in doc.entities().filter(|e| {
        e.common().layer == layer_name
            && e.common().owner_handle == doc.header.model_space_block_handle
    }) {
        classify(e);
    }

    // Exploded block entities on this layer (ByBlock colors already resolved)
    if let Some(exploded) = exploded_by_layer.get(layer_name) {
        for e in exploded {
            classify(e);
        }
    }
    let mut avail: Vec<(usize, &TI)> = texts.iter().enumerate().collect();
    for le in &lines {
        let mut md = f64::MAX;
        let mut np: Option<usize> = None;
        for (pos, (_, ti)) in avail.iter().enumerate() {
            for seg in le.verts.windows(2) {
                let d = calc_h(ti.pos, seg[0], seg[1]);
                if d < md {
                    md = d;
                    np = Some(pos);
                }
            }
        }
        let mut af = le.feats.clone();
        if let Some(pos) = np {
            if md < 2.0 {
                let (_, info) = avail[pos];
                if let Some(mf) = af.first_mut() {
                    if let Some(p) = mf.get_mut("properties").and_then(|p| p.as_object_mut()) {
                        p.insert("tunnelCode".into(), json!(info.text));
                    }
                }
                avail.remove(pos);
            }
        }
        features.extend(af);
    }

    // Output ALL text entities as proper features (preserving fontSize, color, etc.)
    for te in &text_entities {
        if let Some(fs) = entity_to_features(te, doc) {
            features.extend(fs);
        }
    }

    features
}
fn calc_h(tn: Vector3, p1: Vector3, p2: Vector3) -> f64 {
    let a = d2d(p1, p2);
    if a == 0.0 {
        return d2d(p1, tn);
    }
    let b = d2d(p1, tn);
    let c = d2d(p2, tn);
    if (c + b - a).abs() < 1e-6 {
        return 0.0;
    }
    if a * a + b * b >= c * c && a * a + c * c >= b * b {
        let p = (a + b + c) / 2.0;
        let s = (p * (p - a) * (p - b) * (p - c)).abs().sqrt();
        return 2.0 * s / a;
    }
    f64::MAX
}
fn d2d(a: Vector3, b: Vector3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

fn calc_text_rotation(rot: f64, n: Vector3) -> f64 {
    if n.x.abs() < 1.0 / 64.0 && n.y.abs() < 1.0 / 64.0 {
        return rot.to_degrees();
    }
    let nn = vn3(n.x, n.y, n.z);
    let rv = if nn.0.abs() < 1.0 / 64.0 && nn.1.abs() < 1.0 / 64.0 {
        (0.0, 1.0, 0.0)
    } else {
        (0.0, 0.0, 1.0)
    };
    let u = vnn(vcross(rv, nn));
    let v = vcross(nn, u);
    let vw = (
        rot.cos() * u.0 + rot.sin() * v.0,
        rot.cos() * u.1 + rot.sin() * v.1,
        rot.cos() * u.2 + rot.sin() * v.2,
    );
    let mut d = vw.1.atan2(vw.0).to_degrees();
    if d < 0.0 {
        d += 360.0;
    }
    d
}
fn vn3(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let l = (x * x + y * y + z * z).sqrt();
    if l < 1e-12 {
        return (0.0, 0.0, 0.0);
    }
    (x / l, y / l, z / l)
}
fn vcross(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64, f64) {
    (
        a.1 * b.2 - a.2 * b.1,
        a.2 * b.0 - a.0 * b.2,
        a.0 * b.1 - a.1 * b.0,
    )
}
fn vnn(v: (f64, f64, f64)) -> (f64, f64, f64) {
    vn3(v.0, v.1, v.2)
}
