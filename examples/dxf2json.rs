//! DXF/DWG to GeoJSON 转换器
//!
//! 读取 DXF 或 DWG 文件，按图层输出 GeoJSON FeatureCollection。
//! 根据文件扩展名自动选择 DxfReader 或 DwgReader。
//!
//! # 实体转换规则
//!
//! | DXF 实体      | GeoJSON 几何类型   | 说明                                           |
//! |---------------|-------------------|------------------------------------------------|
//! | Line          | LineString        | 起点→终点，2个坐标点                             |
//! | Point         | Point             | 单个坐标点                                      |
//! | Circle        | LineString        | 离散化为60段的闭合线（首尾重合）                   |
//! | Arc           | LineString        | 按6°步长离散化为多段线                            |
//! | Text          | Point             | 插入点作为Point，文字内容/字号/旋转角存入properties |
//! | MText         | Point             | 同Text，多行文字解析为纯文本                       |
//! | LwPolyline    | LineString        | 含bulge弧线段离散化，支持OCS→WCS转换              |
//! | Polyline(3D)  | LineString        | 直接取3D顶点坐标                                  |
//! | Polyline(2D)  | LineString        | 同LwPolyline，含bulge和OCS处理                    |
//! | Ellipse       | LineString        | 参数方程离散化（步长0.05弧度）                      |
//! | Spline        | LineString        | NURBS de Boor算法求值后离散化                      |
//! | Hatch         | MultiPolygon      | 遍历边界路径（Line/Arc/Ellipse/Polyline/Spline边） |
//! | Solid         | Polygon           | 3或4角点闭合多边形                                |
//! | Face3D        | Polygon           | 3或4角点闭合多边形                                |
//! | Insert        | (展开后递归转换)    | 调用explode_from_document展开块引用后递归转换       |
//!
//! # 坐标系处理
//! - 使用 Arbitrary Axis Algorithm（任意轴算法）进行 OCS→WCS 转换
//! - 法向量为 (0,0,1) 时 OCS 等同于 WCS
//!
//! # 输出格式
//! - 每个有实体的图层输出一个 FeatureCollection
//! - 包含 layerName / visible / layerType / crs / features 字段
//! - 输出文件名带时间戳: output_{timestamp}.json

use acadrust::entities::*;
use acadrust::objects::ObjectType;
use acadrust::types::{Color, Matrix3, Vector2, Vector3};
use acadrust::{CadDocument, DwgReader, DxfReader, EntityType};
use serde_json::{json, Map, Value};
use std::env;
use std::fs;
use std::path::Path;

// ── 弧线离散化最小角度步长（弧度），6° ──
const SMALLEST_ANGLE: f64 = 6.0 * std::f64::consts::PI / 180.0;

fn main() -> acadrust::Result<()> {
    let args: Vec<String> = env::args().collect();
    let input_file = if args.len() > 1 {
        args[1].clone()
    } else {
        "E:/home/dxf-gis/tt.dwg".to_string()
    };

    // ── 1. 根据文件扩展名自动选择 DXF 或 DWG 读取器 ──
    let ext = Path::new(&input_file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("dxf")
        .to_lowercase();

    let doc = if ext == "dwg" {
        println!("Reading DWG file: {}", input_file);
        let mut reader = DwgReader::from_file(&input_file)?;
        reader.read()?
    } else {
        println!("Reading DXF file: {}", input_file);
        DxfReader::from_file(&input_file)?.read()?
    };
    println!("Version: {}", doc.version.as_str());
    println!(
        "Layers: {}, Entities: {}",
        doc.layers.iter().count(),
        doc.entities().count()
    );

    // ── 2. 预处理：展开所有 Insert（块引用）实体 ──
    //
    // 块内的实体可能定义在 layer "0"（继承 Insert 的图层）或特定图层（如 AXIS_TEXT）。
    // explode 后这些实体保留各自的图层，但不在文档顶层实体列表中，
    // 因此需要预先展开并按图层归集，才能让 _TEXT 等图层正确输出。
    //
    // Insert 的 attributes（ATTRIB）携带实际属性值和 WCS 坐标，
    // 也需要按图层归集。
    let mut exploded_by_layer: std::collections::HashMap<String, Vec<EntityType>> =
        std::collections::HashMap::new();

    for entity in doc.entities() {
        if let EntityType::Insert(ins) = entity {
            // 展开块定义内的子实体
            let exploded = ins.explode_from_document(&doc);
            for sub_entity in exploded {
                let layer = sub_entity.common().layer.clone();
                exploded_by_layer
                    .entry(layer)
                    .or_default()
                    .push(sub_entity);
            }
            // 收集 Insert 附带的属性实体（ATTRIB）
            for attrib in &ins.attributes {
                let layer = attrib.common.layer.clone();
                exploded_by_layer
                    .entry(layer)
                    .or_default()
                    .push(EntityType::AttributeEntity(attrib.clone()));
            }
        }
    }

    let exploded_by_layer = exploded_by_layer; // immutable from here

    // ── 3. 收集所有图层名 ──
    let layer_names: Vec<String> = doc.layers.iter().map(|l| l.name.clone()).collect();

    // ── 4. 创建 output 目录 ──
    let output_dir = "output";
    fs::create_dir_all(output_dir)?;

    let mut layer_count = 0u32;

    // ── 5. 按图层遍历，每个图层输出一个独立的 JSON 文件 ──
    for layer_name in &layer_names {
        let features = collect_layer_features(&doc, layer_name, &exploded_by_layer);
        if features.is_empty() {
            continue;
        }

        // 获取图层可见性
        let layer = doc.layers.get(layer_name);
        let visible = layer.map(|l| !l.flags.off).unwrap_or(true);
        let is_base_layer = layer_name == "定位基准线";

        // 构建 FeatureCollection（直接作为顶层对象，不再嵌套外层 {}）
        let mut fc = Map::new();
        fc.insert("type".into(), json!("FeatureCollection"));
        fc.insert("layerName".into(), json!(layer_name));
        fc.insert("visible".into(), json!(if visible { "1" } else { "0" }));
        fc.insert("layerType".into(), json!(if is_base_layer { "1" } else { "0" }));

        // 图层 handle code
        if let Some(l) = layer {
            fc.insert("layerCode".into(), json!(format!("L{}", l.handle.value())));
        }

        // 坐标系：优先从 DXF GeoData 读取，读取不到则使用默认值
        let crs = read_crs_from_doc(&doc);
        fc.insert("crs".into(), crs);

        // 过滤空坐标的 feature
        let filtered: Vec<Value> = features.into_iter().filter(|f| !has_empty_coords(f)).collect();
        fc.insert("features".into(), Value::Array(filtered));

        // 输出: output/{layer_name}.json
        let safe_name = layer_name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
        let output_file = format!("{}/{}.json", output_dir, safe_name);
        fs::write(
            &output_file,
            serde_json::to_string_pretty(&Value::Object(fc)).unwrap(),
        )?;
        layer_count += 1;
        println!("Written: {}", output_file);
    }

    println!("Output: {} layer files in {}/", layer_count, output_dir);

    Ok(())
}

/// 收集指定图层的所有 GeoJSON Feature
///
/// 遍历文档中属于该图层的顶层实体 + 预展开的块引用子实体，
/// 按类型分发到对应的转换函数。
///
/// 「定位基准线」图层走特殊逻辑：将 Text 关联到最近的 Line。
fn collect_layer_features(
    doc: &CadDocument,
    layer_name: &str,
    exploded_by_layer: &std::collections::HashMap<String, Vec<EntityType>>,
) -> Vec<Value> {
    // 定位基准线：走 Text-Line 关联逻辑
    if layer_name == "定位基准线" {
        return collect_positioning_baseline_features(doc, layer_name);
    }

    let mut features = Vec::new();

    // 1. 顶层实体（跳过 Insert 和 AttributeDefinition）
    //    Insert 已在 exploded_by_layer 中展开；
    //    ATTDEF 是块内模板，坐标为块局部坐标，不应直接输出。
    for entity in doc.entities().filter(|e| e.common().layer == layer_name) {
        if matches!(entity, EntityType::Insert(_) | EntityType::AttributeDefinition(_)) {
            continue;
        }
        if let Some(fs) = entity_to_features(entity) {
            features.extend(fs);
        }
    }

    // 2. 预展开的块引用子实体
    if let Some(exploded) = exploded_by_layer.get(layer_name) {
        for entity in exploded {
            if let Some(fs) = entity_to_features(entity) {
                features.extend(fs);
            }
        }
    }

    features
}

/// 将单个实体转为 GeoJSON Feature 列表，不支持的类型返回空 Vec。
/// LwPolyline 含箭头段时会产生多个 Feature（LineString + Polygon）。
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
        _ => return None,
    };
    Some(features)
}

// ═══════════════════════════════════════════════════════════════
//  坐标转换辅助函数
// ═══════════════════════════════════════════════════════════════

/// OCS（对象坐标系）→ WCS（世界坐标系）转换
///
/// 使用 Arbitrary Axis Algorithm：
/// - 根据法向量构建 OCS 的 XYZ 基向量矩阵
/// - 将 OCS 中的点通过矩阵乘法转换到 WCS
fn ocs_to_wcs(normal: Vector3, point: Vector3) -> Vector3 {
    let basis = Matrix3::arbitrary_axis(normal);
    basis * point
}

/// Vector3 → JSON 坐标数组 [x, y, z]
fn pt(v: Vector3) -> Value {
    json!([v.x, v.y, v.z])
}

/// Color → 十六进制颜色字符串
///
/// - Rgb { r, g, b } → "#rrggbb"
/// - Index(i) → 查 ACI 颜色表转 RGB
/// - ByLayer / ByBlock → 默认白色/黑色
fn color_to_hex(color: Color) -> String {
    match color {
        Color::Rgb { r, g, b } => format!("#{:02x}{:02x}{:02x}", r, g, b),
        Color::Index(i) => {
            if let Some((r, g, b)) = Color::Index(i).rgb() {
                format!("#{:02x}{:02x}{:02x}", r, g, b)
            } else {
                "#ffffff".to_string()
            }
        }
        Color::ByLayer => "#ffffff".to_string(),
        Color::ByBlock => "#000000".to_string(),
    }
}

/// 构建标准 GeoJSON Feature 对象
fn make_feature(geom_type: &str, coordinates: Value, properties: Value) -> Value {
    json!({
        "type": "Feature",
        "geometry": {
            "type": geom_type,
            "coordinates": coordinates
        },
        "properties": properties
    })
}

/// 构建标准 GeoJSON Feature 对象（含 entity handle code）
fn make_feature_with_code(geom_type: &str, coordinates: Value, properties: Value, code: u64) -> Value {
    json!({
        "type": "Feature",
        "code": code,
        "geometry": {
            "type": geom_type,
            "coordinates": coordinates
        },
        "properties": properties
    })
}

// ═══════════════════════════════════════════════════════════════
//  弧线离散化辅助函数
// ═══════════════════════════════════════════════════════════════

/// 将弧线离散化为点序列
///
/// 参数：圆心(center)、半径(radius)、起止角度(弧度)、法向量(normal)
/// 按 SMALLEST_ANGLE (6°) 步长采样，保证弧线精度
/// 支持 OCS 法向量，通过 arbitrary_axis 转换到 WCS
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

/// 将带 bulge（凸度）的线段离散化为弧线段
///
/// bulge = tan(包含角/4)，正值弧在起点→终点方向的左侧，负值在右侧
/// 计算圆弧中心和半径后，按角度步长采样
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
//  实体 → GeoJSON Feature 转换函数
// ═══════════════════════════════════════════════════════════════

/// Line → LineString
/// 起点(start) → 终点(end)，生成2个坐标点的线段
fn line_to_feature(line: &Line) -> Value {
    let color = color_to_hex(line.common.color);
    make_feature_with_code(
        "LineString",
        json!([pt(line.start), pt(line.end)]),
        json!({"color": color}),
        line.common.handle.value(),
    )
}

/// Point → Point
/// 单个坐标点，经 OCS→WCS 转换
fn point_to_feature(point: &Point) -> Value {
    let color = color_to_hex(point.common.color);
    let wcs = ocs_to_wcs(point.normal, point.location);
    make_feature_with_code("Point", pt(wcs), json!({"color": color}), point.common.handle.value())
}

/// Circle → LineString（闭合）
/// 将圆离散化为60段线段，首尾坐标重合形成闭合圆
/// 支持 OCS 法向量（非标准法向量的圆）
fn circle_to_feature(circle: &Circle) -> Value {
    let color = color_to_hex(circle.common.color);
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

/// Arc → LineString
/// 弧线按6°步长离散化为多段线
/// 在 OCS 中采样后通过 arbitrary_axis 矩阵转换到 WCS
fn arc_to_feature(arc: &Arc) -> Value {
    let color = color_to_hex(arc.common.color);
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
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), arc.common.handle.value())
}

/// Text → Point
/// 文字实体转为点要素（插入点），文字内容、字号、旋转角度存入 properties
/// 旋转角度：当 normal=(0,0,1) 时直接取 rotation；否则通过 OCS→WCS 投影修正
fn text_to_feature(text: &Text) -> Value {
    let color = color_to_hex(text.common.color);
    // DXF 中当文字有对齐方式（非 Left/Baseline）时，
    // alignment_point 才是真正的定位参考点，insertion_point 仅为左下角基准。
    // 优先使用 alignment_point，否则回退到 insertion_point。
    let ref_point = text.alignment_point.unwrap_or(text.insertion_point);
    let wcs = ocs_to_wcs(text.normal, ref_point);
    let rotation_deg = calc_text_rotation(text.rotation, text.normal);
    // 解析 %%c/%%d/%%u 等特殊编码为纯文本
    let plain = acadrust::entities::mtext_format::parse_plain_text(&text.value);
    let display_text = plain.to_plain_text();

    // 对齐方式映射：水平 + 垂直
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

    make_feature_with_code(
        "Point",
        pt(wcs),
        json!({
            "color": color,
            "text": display_text,
            "fontSize": text.height,
            "rotation": rotation_deg,
            "textAlign": h_align,
            "textBaseline": v_align
        }),
        text.common.handle.value(),
    )
}

/// MText → Point
/// 多行文字实体解析格式化标记后提取纯文本
/// 对齐方式(attachment_point) 存入 properties
fn mtext_to_feature(mtext: &MText) -> Value {
    let color = color_to_hex(mtext.common.color);
    let wcs = ocs_to_wcs(mtext.normal, mtext.insertion_point);
    let rotation_deg = calc_text_rotation(mtext.rotation, mtext.normal);
    // 解析 MText 格式化字符串（\A1;{\f...}等）为纯文本
    let doc = acadrust::entities::mtext_format::parse_mtext(&mtext.value, true);
    let display_text = doc.to_plain_text();
    let alignment = mtext.attachment_point as i32;
    make_feature_with_code(
        "Point",
        pt(wcs),
        json!({
            "color": color,
            "text": display_text,
            "fontSize": mtext.height,
            "rotation": rotation_deg,
            "align": alignment
        }),
        mtext.common.handle.value(),
    )
}

/// AttributeEntity → Point
/// 块属性实例（插入块后的实际属性值），输出为点要素。
/// 文字内容取 value（实际属性值）。
/// 当对齐方式非 Left/Baseline 时，使用 alignment_point 作为定位参考点。
fn attrib_to_feature(attrib: &AttributeEntity) -> Value {
    let color = color_to_hex(attrib.common.color);
    // 对齐方式非默认时，alignment_point 才是真正的定位参考点
    let is_default_align = matches!(
        (attrib.horizontal_alignment, attrib.vertical_alignment),
        (acadrust::entities::attribute_definition::HorizontalAlignment::Left,
         acadrust::entities::attribute_definition::VerticalAlignment::Baseline)
    );
    let ref_point = if is_default_align {
        attrib.insertion_point
    } else {
        attrib.alignment_point
    };
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

    make_feature_with_code(
        "Point",
        pt(wcs),
        json!({
            "color": color,
            "text": attrib.value,
            "fontSize": attrib.height,
            "rotation": rotation_deg,
            "textAlign": h_align,
            "textBaseline": v_align
        }),
        attrib.common.handle.value(),
    )
}

/// LwPolyline → 多个 GeoJSON Feature
///
/// 1. 原始 LineString（含 widths 元数据）
/// 2. 箭头段转换为闭合填充 Polygon（宽渐变段）
fn lwpolyline_to_features(pl: &LwPolyline) -> Vec<Value> {
    let color = color_to_hex(pl.common.color);
    let mut features = Vec::new();
    let verts = &pl.vertices;
    let n = verts.len();
    if n == 0 {
        features.push(make_feature("LineString", Value::Array(vec![]), json!({"color": color})));
        return features;
    }

    let normal = pl.normal;
    let basis = Matrix3::arbitrary_axis(normal);

    // ── 1. 生成原始 LineString ──
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

    let mut props = Map::new();
    props.insert("color".into(), json!(color));

    let has_widths = verts.iter().any(|v| v.start_width != 0.0 || v.end_width != 0.0);
    if has_widths {
        let widths: Vec<Value> = verts.iter()
            .map(|v| json!([v.start_width, v.end_width]))
            .collect();
        props.insert("widths".into(), Value::Array(widths));
    }
    if pl.constant_width != 0.0 {
        props.insert("constantWidth".into(), json!(pl.constant_width));
    }

    features.push(make_feature_with_code(
        "LineString", Value::Array(coords), Value::Object(props), pl.common.handle.value(),
    ));

    // ── 2. 箭头段 → 闭合填充 Polygon ──
    // 检测宽度渐变段，生成三角形多边形
    //
    // 三角形从宽端到窄端自然收尖：
    //   底边在宽端（宽度>0），尖端在窄端（宽度=0）
    //
    // - sw > ew：宽端在 v[i]（p0），底边在 p0，尖端在 p1（v[i+1]）
    // - sw < ew：宽端在 v[i+1]（p1），底边在 p1，尖端在 p0（v[i]）
    for i in 0..n.saturating_sub(1) {
        let v0 = &verts[i];
        let v1 = &verts[i + 1];
        let sw = v0.start_width;
        let ew = v0.end_width;
    
        // 跳过等宽段和零宽段
        if (sw - ew).abs() < 1e-6 || (sw == 0.0 && ew == 0.0) {
            continue;
        }
    
        // 计算 OCS 中的线段方向
        let p0 = v0.location; // Vector2
        let p1 = v1.location;
        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let seg_len = (dx * dx + dy * dy).sqrt();
        if seg_len < 1e-10 {
            continue;
        }
    
        // 垂直方向（单位向量）
        let nx = -dy / seg_len;
        let ny = dx / seg_len;
    
        // 根据宽度渐变方向生成三角形
        // 底边在宽端，尖端在窄端，三角形从宽到窄自然收尖
        let corners_2d: Vec<Vector2> = if sw > ew {
            // sw > ew：宽端在 v[i]（p0），底边在 p0，尖端在 p1
            let half_sw = sw / 2.0;
            vec![
                Vector2::new(p0.x + half_sw * nx, p0.y + half_sw * ny), // 底边+perp
                Vector2::new(p0.x - half_sw * nx, p0.y - half_sw * ny), // 底边-perp
                p1, // 尖端（窄端）
            ]
        } else {
            // sw < ew：宽端在 v[i+1]（p1），底边在 p1，尖端在 p0
            let half_ew = ew / 2.0;
            vec![
                Vector2::new(p1.x + half_ew * nx, p1.y + half_ew * ny), // 底边+perp
                Vector2::new(p1.x - half_ew * nx, p1.y - half_ew * ny), // 底边-perp
                p0, // 尖端（窄端）
            ]
        };

        // OCS 2D → WCS 3D
        let corners_wcs: Vec<Vector3> = corners_2d.iter()
            .map(|c| basis * Vector3::new(c.x, c.y, pl.elevation))
            .collect();

        // 构建闭合 Polygon 坐标
        let mut ring: Vec<Value> = corners_wcs.iter().map(|c| pt(*c)).collect();
        ring.push(pt(corners_wcs[0])); // 闭合

        let arrow_props = json!({
            "color": color,
            "arrow": true,
            "segment": i,
            "startWidth": sw,
            "endWidth": ew,
        });

        features.push(make_feature_with_code(
            "Polygon",
            json!([ring]),
            arrow_props,
            pl.common.handle.value(),
        ));
    }

    features
}

/// Polyline(3D) → LineString
/// 三维多段线，顶点已包含3D坐标，直接输出
fn polyline3d_to_feature(pl: &Polyline) -> Value {
    let color = color_to_hex(pl.common.color);
    let coords: Vec<Value> = pl.vertices.iter().map(|v| pt(v.location)).collect();
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), pl.common.handle.value())
}

/// Polyline(2D) → LineString
/// 二维多段线，处理逻辑同 LwPolyline：
/// - 含 bulge 的边离散化为弧线
/// - OCS→WCS 坐标转换
/// - 支持闭合
fn polyline2d_to_feature(pl: &Polyline2D) -> Value {
    let color = color_to_hex(pl.common.color);
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

    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), pl.common.handle.value())
}

/// Ellipse → LineString
/// 椭圆（或椭圆弧）通过参数方程离散化：
/// - major_axis 确定长轴方向和长度
/// - minor_axis_ratio 确定短轴
/// - start/end_parameter 为参数角度（弧度）
/// - 椭圆已在 WCS 中，无需 OCS 转换
fn ellipse_to_feature(ellipse: &Ellipse) -> Value {
    let color = color_to_hex(ellipse.common.color);
    let center = ellipse.center;
    let major = ellipse.major_axis;
    let major_len = major.length();
    let minor_len = major_len * ellipse.minor_axis_ratio;

    // 计算椭圆平面内的基向量
    let u = major * (1.0 / major_len.max(1e-12)); // 长轴方向单位向量
    let v = ellipse.normal.cross(&u); // 短轴方向单位向量

    let start_param = ellipse.start_parameter;
    let mut end_param = ellipse.end_parameter;
    if end_param <= start_param {
        end_param += 2.0 * std::f64::consts::PI;
    }

    // 步长 0.05 弧度（约2.86°）
    let delta = 0.05;
    let mut coords = Vec::new();
    let mut t = start_param;
    while t <= end_param + delta {
        if t > end_param {
            t = end_param;
        }
        // 参数方程: P(t) = center + u*a*cos(t) + v*b*sin(t)
        let p = center + u * (major_len * t.cos()) + v * (minor_len * t.sin());
        coords.push(pt(p));
        if (t - end_param).abs() < 1e-12 {
            break;
        }
        t += delta;
    }

    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), ellipse.common.handle.value())
}

/// Spline → LineString
/// NURBS 样条曲线通过 de Boor 算法求值后离散化
/// - 支持有理（带权重）和非有理 B-spline
/// - 采样数 = max(控制点数*10, 20)，上限1000
fn spline_to_feature(spline: &Spline) -> Value {
    let color = color_to_hex(spline.common.color);
    let pts = evaluate_nurbs(spline);
    let coords: Vec<Value> = pts.iter().map(|p| pt(*p)).collect();
    make_feature_with_code("LineString", Value::Array(coords), json!({"color": color}), spline.common.handle.value())
}

/// NURBS 曲线求值（de Boor 算法）
///
/// 支持有理（权重）和非有理 B-spline：
/// 1. 确定参数范围 [t_min, t_max]
/// 2. 均匀采样，对每个参数值 t：
///    a. 找到 t 所在的节点区间（knot span）
///    b. 用 de Boor 递推计算曲线点
///    c. 有理情况除以权重得到最终坐标
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

    // 确定参数范围
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
        let t = t_min + i as f64 * step;
        let t = t.min(t_max);

        // 找到 t 所在的节点区间 [knots[span], knots[span+1])
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

        // de Boor 递推
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

        // 有理化：除以最终权重
        let w_final = w[degree];
        if w_final.abs() > 1e-14 {
            result.push(d[degree] * (1.0 / w_final));
        } else {
            result.push(d[degree]);
        }
    }

    result
}

/// Hatch → MultiPolygon
///
/// 填充实体的边界路径转多边形：
/// - 每个 BoundaryPath 对应一个多边形环
/// - 边界边类型：
///   - Line: 直线段（起点→终点）
///   - CircularArc: 圆弧段（按6°步长离散化）
///   - EllipticArc: 椭圆弧段（参数方程离散化）
///   - Polyline: 多段线（含bulge弧段处理）
///   - Spline: 样条（简化为控制点连线）
/// - 所有边在 OCS 中处理后通过 arbitrary_axis 转 WCS
fn hatch_to_feature(hatch: &Hatch) -> Value {
    let color = color_to_hex(hatch.common.color);
    let mut polygon_coords: Vec<Value> = Vec::new();

    let basis = Matrix3::arbitrary_axis(hatch.normal);

    for path in &hatch.paths {
        let mut ring: Vec<Value> = Vec::new();

        for edge in &path.edges {
            match edge {
                // 直线边：直接连接起点→终点
                BoundaryEdge::Line(line_edge) => {
                    let s = basis * Vector3::new(line_edge.start.x, line_edge.start.y, 0.0);
                    let e = basis * Vector3::new(line_edge.end.x, line_edge.end.y, 0.0);
                    if ring.is_empty() {
                        ring.push(pt(s));
                    }
                    ring.push(pt(e));
                }
                // 圆弧边：圆心+半径+起止角度，按6°步长离散化
                BoundaryEdge::CircularArc(arc_edge) => {
                    let center = arc_edge.center;
                    let r = arc_edge.radius;
                    let mut sa = arc_edge.start_angle;
                    let mut ea = arc_edge.end_angle;
                    if ea <= sa {
                        ea += 2.0 * std::f64::consts::PI;
                    }
                    if !arc_edge.counter_clockwise {
                        std::mem::swap(&mut sa, &mut ea);
                        ea += 2.0 * std::f64::consts::PI;
                    }
                    let sweep = ea - sa;
                    let segments = (sweep / SMALLEST_ANGLE).ceil().max(1.0) as usize;
                    let step = sweep / segments as f64;

                    if ring.is_empty() {
                        let local = Vector3::new(r * sa.cos(), r * sa.sin(), 0.0);
                        let wcs_pt =
                            basis * (Vector3::new(center.x, center.y, 0.0) + local);
                        ring.push(pt(wcs_pt));
                    }

                    for i in 1..=segments {
                        let angle = sa + i as f64 * step;
                        let local = Vector3::new(r * angle.cos(), r * angle.sin(), 0.0);
                        let wcs_pt =
                            basis * (Vector3::new(center.x, center.y, 0.0) + local);
                        ring.push(pt(wcs_pt));
                    }
                }
                // 椭圆弧边：参数方程离散化
                BoundaryEdge::EllipticArc(ell_edge) => {
                    let center = ell_edge.center;
                    let major_ep = ell_edge.major_axis_endpoint;
                    let major_len = major_ep.length();
                    let minor_len = major_len * ell_edge.minor_axis_ratio;
                    // 在2D平面内构建椭圆基向量
                    let u3 = Vector3::new(major_ep.x, major_ep.y, 0.0)
                        * (1.0 / major_len.max(1e-12));
                    let v3 = Vector3::new(-u3.y, u3.x, 0.0); // 垂直方向

                    let sa = ell_edge.start_angle;
                    let mut ea = ell_edge.end_angle;
                    if ea <= sa {
                        ea += 2.0 * std::f64::consts::PI;
                    }
                    let sweep = ea - sa;
                    let segments = (sweep / 0.05).ceil().max(1.0) as usize;
                    let step = sweep / segments as f64;

                    if ring.is_empty() {
                        let local =
                            u3 * (major_len * sa.cos()) + v3 * (minor_len * sa.sin());
                        let wcs_pt =
                            basis * (Vector3::new(center.x, center.y, 0.0) + local);
                        ring.push(pt(wcs_pt));
                    }

                    for i in 1..=segments {
                        let t = sa + i as f64 * step;
                        let local =
                            u3 * (major_len * t.cos()) + v3 * (minor_len * t.sin());
                        let wcs_pt =
                            basis * (Vector3::new(center.x, center.y, 0.0) + local);
                        ring.push(pt(wcs_pt));
                    }
                }
                // 多段线边：顶点含 bulge，弧段离散化处理
                // PolylineEdge.vertices 为 Vec<Vector3>，其中 z 分量存储 bulge 值
                BoundaryEdge::Polyline(poly_edge) => {
                    let verts = &poly_edge.vertices;
                    let nv = verts.len();
                    for i in 0..nv {
                        let v = &verts[i];
                        let bulge = v.z; // z 存储 bulge 值
                        let start = Vector2::new(v.x, v.y);

                        if bulge.abs() > 1e-10 && i < nv - 1 {
                            let next = &verts[(i + 1) % nv];
                            let end = Vector2::new(next.x, next.y);
                            let arc_pts = tessellate_bulge(start, end, bulge);
                            for p in &arc_pts {
                                let wcs_pt = basis * Vector3::new(p.x, p.y, 0.0);
                                ring.push(pt(wcs_pt));
                            }
                        } else {
                            let wcs_pt = basis * Vector3::new(start.x, start.y, 0.0);
                            if ring.is_empty() || i > 0 {
                                ring.push(pt(wcs_pt));
                            } else if ring.is_empty() {
                                ring.push(pt(wcs_pt));
                            }
                        }
                    }
                    // 闭合多段线
                    if poly_edge.is_closed && ring.len() > 1 {
                        ring.push(ring[0].clone());
                    }
                }
                // 样条边：简化处理，直接使用控制点
                BoundaryEdge::Spline(spline_edge) => {
                    for cp in &spline_edge.control_points {
                        let wcs_pt = basis * Vector3::new(cp.x, cp.y, 0.0);
                        ring.push(pt(wcs_pt));
                    }
                }
            }
        }

        // 确保环闭合
        if ring.len() > 1 && ring.first() != ring.last() {
            ring.push(ring[0].clone());
        }

        if !ring.is_empty() {
            polygon_coords.push(Value::Array(vec![Value::Array(ring)]));
        }
    }

    make_feature_with_code(
        "MultiPolygon",
        Value::Array(polygon_coords),
        json!({"color": color}),
        hatch.common.handle.value(),
    )
}

/// Solid → Polygon
/// 2D 填充实体，3或4个角点构成闭合多边形
/// 第3角和第4角重合时为三角形
///
/// 注意：DXF 规范中 Solid 的顶点存储顺序不是环绕顺序：
///   first_corner=角点1, second_corner=角点2,
///   third_corner=角点4（对角）, fourth_corner=角点3
/// 正确的多边形环绕顺序为: p1 → p2 → p4 → p3
fn solid_to_feature(solid: &Solid) -> Value {
    let color = color_to_hex(solid.common.color);
    let basis = Matrix3::arbitrary_axis(solid.normal);
    let p1 = basis * solid.first_corner;
    let p2 = basis * solid.second_corner;
    let p3 = basis * solid.third_corner;   // DXF 中这是角点4（对角位置）
    let p4 = basis * solid.fourth_corner;  // DXF 中这是角点3

    let mut ring = vec![pt(p1), pt(p2)];
    if !solid.is_triangle() {
        // 四边形：环绕顺序 p1 → p2 → p4(角点3) → p3(角点4)
        ring.push(pt(p4));
        ring.push(pt(p3));
    } else {
        // 三角形：third_corner 和 fourth_corner 重合
        ring.push(pt(p3));
    }
    ring.push(ring[0].clone()); // 闭合

    make_feature_with_code("Polygon", json!([ring]), json!({"color": color}), solid.common.handle.value())
}

/// Face3D → Polygon
/// 3D 面实体，3或4个角点构成闭合多边形
/// 与 Solid 类似，但顶点已在 WCS 中
fn face3d_to_feature(face: &Face3D) -> Value {
    let color = color_to_hex(face.common.color);
    let mut ring = vec![
        pt(face.first_corner),
        pt(face.second_corner),
        pt(face.third_corner),
    ];
    if !face.is_triangle() {
        ring.push(pt(face.fourth_corner));
    }
    ring.push(ring[0].clone()); // 闭合

    make_feature_with_code("Polygon", json!([ring]), json!({"color": color}), face.common.handle.value())
}

// ═══════════════════════════════════════════════════════════════
//  空坐标过滤
// ═══════════════════════════════════════════════════════════════

/// 检查 Feature 是否包含空坐标（递归检查）
/// 用于过滤无效的空几何体
fn has_empty_coords(feature: &Value) -> bool {
    if let Some(geom) = feature.get("geometry") {
        if let Some(coords) = geom.get("coordinates") {
            return check_empty_coords(coords);
        }
        return true; // 无 coordinates 字段
    }
    true // 无 geometry 字段
}

/// 递归检查坐标数组是否为空
fn check_empty_coords(val: &Value) -> bool {
    match val {
        Value::Array(arr) => {
            if arr.is_empty() {
                return true;
            }
            arr.iter().any(|v| check_empty_coords(v))
        }
        _ => false,
    }
}

// ═════════════════════════════════════════════════════════════
//  CRS 自动读取
// ═════════════════════════════════════════════════════════════

/// 从 DXF 文件的 GeoData 对象中读取 CRS 信息
///
/// 遍历文档中的 objects，查找 GeoData 对象，提取 coordinate_system_definition 字段。
/// 支持两种格式：
/// - WKT (PROJCS[...])：提取 EPSG 码或 CRS 名称
/// - MapGuide XML：提取 CS_NAME 或 EPSG 码
/// 读取失败时回退到默认 EPSG:900913
fn read_crs_from_doc(doc: &CadDocument) -> Value {
    let default_crs = json!({
        "type": "name",
        "properties": { "name": "urn:ogc:def:crs:EPSG::900913" }
    });

    // 遍历文档对象，查找 GeoData
    for obj in doc.objects.values() {
        if let ObjectType::GeoData(gd) = obj {
            let cs_def = &gd.coordinate_system_definition;
            if cs_def.is_empty() {
                continue;
            }

            println!("[CRS] Found GeoData, coordinate_type={}, cs_def length={}",
                gd.coordinate_type, cs_def.len());

            // 尝试从 WKT 格式提取 EPSG 码
            // 示例: PROJCS["...", AUTHORITY["EPSG","3857"]]
            if let Some(epsg) = extract_epsg_from_wkt(cs_def) {
                println!("[CRS] Extracted EPSG from WKT: {}", epsg);
                return json!({
                    "type": "name",
                    "properties": { "name": format!("urn:ogc:def:crs:EPSG::{}", epsg) }
                });
            }

            // 尝试从 MapGuide XML 提取 CRS 名称
            // 示例: <CS_NAME>WGS84.PseudoMercator</CS_NAME> 或 <EPSG_CODE>3857</EPSG_CODE>
            if let Some(crs_name) = extract_crs_from_xml(cs_def) {
                println!("[CRS] Extracted CRS from XML: {}", crs_name);
                return json!({
                    "type": "name",
                    "properties": { "name": crs_name }
                });
            }

            // 有 GeoData 但无法解析 CRS 定义
            println!("[CRS] GeoData found but could not parse CRS definition, using default");
        }
    }

    println!("[CRS] No GeoData found, using default EPSG:900913");
    default_crs
}

/// 从 WKT 字符串中提取 EPSG 码
///
/// 匹配模式: AUTHORITY["EPSG","xxxx"] 或 AUTHORITY["epsg","xxxx"]
fn extract_epsg_from_wkt(wkt: &str) -> Option<String> {
    // 查找 AUTHORITY["EPSG"," 模式
    let wkt_upper = wkt.to_uppercase();
    let pattern = "AUTHORITY[\"EPSG\",\"";
    if let Some(pos) = wkt_upper.find(pattern) {
        let start = pos + pattern.len();
        let rest = &wkt[start..];
        if let Some(end) = rest.find('"') {
            let epsg = &rest[..end];
            return Some(epsg.to_string());
        }
    }
    None
}

/// 从 MapGuide XML 字符串中提取 CRS 名称
///
/// 优先提取 EPSG_CODE，其次提取 CS_NAME
fn extract_crs_from_xml(xml: &str) -> Option<String> {
    // 尝试提取 EPSG_CODE
    if let Some(code) = extract_xml_tag_value(xml, "EPSG_CODE") {
        return Some(format!("urn:ogc:def:crs:EPSG::{}", code));
    }
    // 尝试提取 CS_NAME
    if let Some(name) = extract_xml_tag_value(xml, "CS_NAME") {
        return Some(format!("urn:ogc:def:crs:EPSG::{}", name));
    }
    None
}

/// 从 XML 字符串中提取指定标签的文本值
///
/// 简单字符串匹配，不依赖 XML 解析库
fn extract_xml_tag_value(xml: &str, tag: &str) -> Option<String> {
    let open_tag = format!("<{}>", tag);
    let close_tag = format!("</{}>", tag);
    if let Some(start) = xml.find(&open_tag) {
        let value_start = start + open_tag.len();
        if let Some(end) = xml[value_start..].find(&close_tag) {
            return Some(xml[value_start..value_start + end].trim().to_string());
        }
    }
    None
}

// ═════════════════════════════════════════════════════════════
//  定位基准线 Text-Line 关联
// ═════════════════════════════════════════════════════════════

/// 定位基准线图层的特殊处理
///
/// 核心逻辑：将该图层上的 Text/MText 关联到最近的 Line。
/// 依据：CAD 制图时 Text 的插入点会吸附到 Line 的端点或线段上，
/// 通过计算 Text 插入点到 Line 起点/终点的距离，找最近的 Text 关联。
///
/// 输出：每条 Line 生成一个 LineString Feature，properties 中包含关联的 Text 信息。
///        独立的 Text 生成 Point Feature（保留未关联的文字）。
fn collect_positioning_baseline_features(doc: &CadDocument, layer_name: &str) -> Vec<Value> {
    let mut features = Vec::new();

    // 1. 收集该图层的所有 Text/MText 实体（位置 + 文本内容）
    struct TextInfo {
        position: Vector3,
        text: String,
    }

    /// 线状实体的抽象：所有顶点 + 已生成的 GeoJSON Feature 列表
    /// features[0] 为主几何体（LineString），后续为箭头 Polygon 等附加几何
    struct LineEntity {
        vertices: Vec<Vector3>,
        features: Vec<Value>,
    }

    let mut texts: Vec<TextInfo> = Vec::new();
    let mut line_entities: Vec<LineEntity> = Vec::new();

    for entity in doc.entities().filter(|e| e.common().layer == layer_name) {
        match entity {
            EntityType::Text(e) => {
                let ref_point = e.alignment_point.unwrap_or(e.insertion_point);
                let wcs = ocs_to_wcs(e.normal, ref_point);
                let plain = acadrust::entities::mtext_format::parse_plain_text(&e.value);
                texts.push(TextInfo {
                    position: wcs,
                    text: plain.to_plain_text(),
                });
            }
            EntityType::MText(e) => {
                let wcs = ocs_to_wcs(e.normal, e.insertion_point);
                let doc = acadrust::entities::mtext_format::parse_mtext(&e.value, true);
                texts.push(TextInfo {
                    position: wcs,
                    text: doc.to_plain_text(),
                });
            }
            EntityType::Line(e) => {
                let feature = line_to_feature(e);
                line_entities.push(LineEntity {
                    vertices: vec![e.start, e.end],
                    features: vec![feature],
                });
            }
            EntityType::LwPolyline(e) => {
                let features = lwpolyline_to_features(e);
                let basis = Matrix3::arbitrary_axis(e.normal);
                let vertices: Vec<Vector3> = e.vertices.iter().map(|v| {
                    basis * Vector3::new(v.location.x, v.location.y, e.elevation)
                }).collect();
                line_entities.push(LineEntity { vertices, features });
            }
            EntityType::Polyline2D(e) => {
                let feature = polyline2d_to_feature(e);
                let basis = Matrix3::arbitrary_axis(e.normal);
                let vertices: Vec<Vector3> = e.vertices.iter().map(|v| {
                    basis * Vector3::new(v.location.x, v.location.y, e.elevation)
                }).collect();
                line_entities.push(LineEntity { vertices, features: vec![feature] });
            }
            _ => {} // 其他实体类型在定位基准线图层中忽略
        }
    }

    println!(
        "[PositioningBaseline] Layer '{}': {} line-entities, {} texts",
        layer_name,
        line_entities.len(),
        texts.len()
    );

    // 2. 遍历每条线实体，找最近的 Text 关联
    //    核心逻辑：计算 Text 插入点到线的每个线段的垂直距离（点到线段距离），取最小值
    //    阈值 < 2.0 时才关联，匹配后从池中移除防止重复关联（参考 Java lbq.remove）
    const MATCH_THRESHOLD: f64 = 2.0;
    let mut available_texts: Vec<(usize, &TextInfo)> = texts.iter().enumerate().collect();

    for le in &line_entities {
        let mut min_dist = f64::MAX;
        let mut nearest_pos: Option<usize> = None;

        for (pos, (_, text_info)) in available_texts.iter().enumerate() {
            let tp = text_info.position;
            // 遍历每对相邻顶点构成的线段，计算点到线段距离
            for seg in le.vertices.windows(2) {
                let dist = calc_height(tp, seg[0], seg[1]);
                if dist < min_dist {
                    min_dist = dist;
                    nearest_pos = Some(pos);
                }
            }
            // Line 只有 2 个顶点，windows(2) 已经覆盖
        }

        // 克隆所有 Feature，仅在阈值内才向主几何体（第一个 Feature）注入关联 Text 属性
        let mut all_features = le.features.clone();
        if let Some(pos) = nearest_pos {
            if min_dist < MATCH_THRESHOLD {
                let (text_idx, text_info) = available_texts[pos];
                if let Some(main_feature) = all_features.first_mut() {
                    if let Some(props) = main_feature.get_mut("properties") {
                        if let Some(obj) = props.as_object_mut() {
                            obj.insert("tunnelCode".into(), json!(text_info.text));
                        }
                    }
                }
                println!("[PositioningBaseline] Matched: text[{}] '{}' <-> line (dist={:.4})",
                    text_idx, text_info.text, min_dist);
                available_texts.remove(pos);
            }
        }
        features.extend(all_features);
    }

    println!(
        "[PositioningBaseline] Result: {} features (matched: {}, unassociated texts: {})",
        features.len(),
        texts.len() - available_texts.len(),
        available_texts.len()
    );

    features
}

/// 计算点到线段的距离（等价于 Java 的 calcHeight）
///
/// 参数：tn=Text插入点，p1=线段起点，p2=线段终点
///
/// 逻辑：
/// - 线段长度为 0 时，返回点到 p1 的距离
/// - 点在线段上时，返回 0
/// - 点的投影落在线段范围内（锐角三角形）时，返回垂直距离（海伦公式求高）
/// - 投影落在线段延长线上时，返回 f64::MAX（不参与匹配）
fn calc_height(tn: Vector3, p1: Vector3, p2: Vector3) -> f64 {
    let a = distance_2d(p1, p2); // 线段长度
    if a == 0.0 {
        return distance_2d(p1, tn);
    }
    let b = distance_2d(p1, tn); // p1 到点的距离
    let c = distance_2d(p2, tn); // p2 到点的距离

    // 点在线段上（浮点容差）
    if (c + b - a).abs() < 1e-6 {
        return 0.0;
    }

    // 只有投影落在线段范围内（组成锐角三角形）才计算
    if a * a + b * b >= c * c && a * a + c * c >= b * b {
        // 海伦公式求面积
        let p = (a + b + c) / 2.0;
        let s = (p * (p - a) * (p - b) * (p - c)).abs().sqrt();
        // 利用面积求高：面积 = 0.5 * 底 * 高
        return 2.0 * s / a;
    }

    // 投影落在线段延长线上，不参与匹配
    f64::MAX
}

/// 计算两点之间的 2D 距离（忽略 Z 轴）
fn distance_2d(a: Vector3, b: Vector3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    (dx * dx + dy * dy).sqrt()
}

// ═════════════════════════════════════════════════════════════
//  OCS 文字旋转角度修正
// ═════════════════════════════════════════════════════════════

/// 将 OCS 中的文字旋转角度转换为 WCS 显示角度
///
/// 当 normal=(0,0,1) 时 OCS 与 WCS 重合，直接返回弧度转度数。
/// 当 normal≠(0,0,1) 时，需要将 OCS 中的角度方向向量投影到 WCS 平面后重新计算角度。
/// 参考 Java 版本 calcOCSAngle 方法。
fn calc_text_rotation(rotation_rad: f64, normal: Vector3) -> f64 {
    let nx = normal.x;
    let ny = normal.y;
    let nz = normal.z;

    // normal ≈ (0,0,±1) 时 OCS 与 WCS 重合，无需转换
    if nx.abs() < 1.0 / 64.0 && ny.abs() < 1.0 / 64.0 {
        return rotation_rad.to_degrees();
    }

    // 1. 构建 OCS 正交基 (Arbitrary Axis Algorithm)
    let n = vec3_normalize(nx, ny, nz);
    let ref_vec = if n.0.abs() < 1.0 / 64.0 && n.1.abs() < 1.0 / 64.0 {
        (0.0, 1.0, 0.0) // N 近似平行 Y 轴，用 Y 做参考
    } else {
        (0.0, 0.0, 1.0) // 默认用 Z 做参考
    };
    let u = vec3_normalize_vec(vec3_cross(ref_vec, n));
    let v = vec3_cross(n, u);

    // 2. OCS 角度 → 单位方向向量
    let theta = rotation_rad;
    let v_ocs = (theta.cos(), theta.sin(), 0.0);

    // 3. 将 OCS 方向向量转回 WCS: v_wcs = v_ocs.x * U + v_ocs.y * V
    let v_wcs = (
        v_ocs.0 * u.0 + v_ocs.1 * v.0,
        v_ocs.0 * u.1 + v_ocs.1 * v.1,
        v_ocs.0 * u.2 + v_ocs.1 * v.2,
    );

    // 4. 在 WCS XY 平面上计算角度
    let mut angle_deg = v_wcs.1.atan2(v_wcs.0).to_degrees();
    if angle_deg < 0.0 {
        angle_deg += 360.0;
    }
    angle_deg
}

/// 3D 向量归一化
fn vec3_normalize(x: f64, y: f64, z: f64) -> (f64, f64, f64) {
    let len = (x * x + y * y + z * z).sqrt();
    if len < 1e-12 {
        return (0.0, 0.0, 0.0);
    }
    (x / len, y / len, z / len)
}

/// 3D 向量叉乘
fn vec3_cross(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64, f64) {
    (
        a.1 * b.2 - a.2 * b.1,
        a.2 * b.0 - a.0 * b.2,
        a.0 * b.1 - a.1 * b.0,
    )
}

/// 3D 向量归一化（元组版本）
fn vec3_normalize_vec(v: (f64, f64, f64)) -> (f64, f64, f64) {
    vec3_normalize(v.0, v.1, v.2)
}
