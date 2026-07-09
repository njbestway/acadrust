//! DXF/DWG to MVT (Mapbox Vector Tile) 预生成器
//!
//! 读取 DXF 或 DWG 文件，生成矢量瓦片 (.pbf) 文件。
//! 输出目录结构: {output_dir}/{z}/{x}/{y}.pbf
//!
//! # 坐标系统
//!
//! CAD 文件坐标通常是投影坐标（米），本工具将其视为"世界坐标"直接映射到瓦片系统。
//! 用户可通过 --bbox 参数指定数据的包围盒（与 DXF 同坐标系），或自动从数据计算。
//!
//! # 用法
//!
//! ```bash
//! cargo run --example dxf2mvt -- [OPTIONS] <input_file>
//!
//! # 选项:
//! #   --min-zoom <N>     最小缩放级别 (默认 0)
//! #   --max-zoom <N>     最大缩放级别 (默认 18)
//! #   --output <DIR>     输出目录 (默认 ./tiles)
//! #   --bbox <W,S,E,N>   手动指定包围盒 (覆盖自动计算)
//! #   --extent <N>       瓦片像素尺寸 (默认 4096)
//! ```

use acadrust::entities::*;
use acadrust::types::{Matrix3, Vector2, Vector3};
use acadrust::{CadDocument, DwgReader, DxfReader, EntityType};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

// Import MVT module
use acadrust::mvt::{
    clip_linestring, clip_point, clip_polygon, covering_tiles, encode_tile, LayerBuilder, BBox,
};

// ── 弧线离散化最小角度步长（弧度），6° ──
const SMALLEST_ANGLE: f64 = 6.0 * std::f64::consts::PI / 180.0;

// ── 简化几何类型 ──

/// 简化的几何表示，用于瓦片裁剪和编码。
#[derive(Debug, Clone)]
enum SimpleGeom {
    Point(f64, f64),
    LineString(Vec<(f64, f64)>),
    Polygon(Vec<Vec<(f64, f64)>>),
}

/// 简化的 Feature 结构。
#[derive(Debug, Clone)]
struct SimpleFeature {
    geom: SimpleGeom,
    layer: String,
    properties: Vec<(String, String)>,
}

// ── 命令行参数 ──

struct Args {
    input_file: String,
    min_zoom: u32,
    max_zoom: u32,
    output_dir: String,
    bbox: Option<BBox>,
    extent: u32,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut input_file = "E:/home/dxf-gis/tt.dwg".to_string();
    let mut min_zoom = 0u32;
    let mut max_zoom = 18u32;
    let mut output_dir = "./tiles".to_string();
    let mut bbox = None;
    let mut extent = 4096u32;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--min-zoom" => {
                i += 1;
                min_zoom = args[i].parse().unwrap_or(0);
            }
            "--max-zoom" => {
                i += 1;
                max_zoom = args[i].parse().unwrap_or(18);
            }
            "--output" => {
                i += 1;
                output_dir = args[i].clone();
            }
            "--bbox" => {
                i += 1;
                let parts: Vec<f64> = args[i].split(',').filter_map(|s| s.parse().ok()).collect();
                if parts.len() == 4 {
                    bbox = Some(BBox::new(parts[0], parts[1], parts[2], parts[3]));
                }
            }
            "--extent" => {
                i += 1;
                extent = args[i].parse().unwrap_or(4096);
            }
            _ if !args[i].starts_with("--") => {
                input_file = args[i].clone();
            }
            _ => {}
        }
        i += 1;
    }

    Args {
        input_file,
        min_zoom,
        max_zoom,
        output_dir,
        bbox,
        extent,
    }
}

// ── 主函数 ──

fn main() -> acadrust::Result<()> {
    let args = parse_args();

    // 1. 读取 CAD 文件
    let ext = Path::new(&args.input_file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("dxf")
        .to_lowercase();

    println!("Reading: {}", args.input_file);
    let doc = if ext == "dwg" {
        let mut reader = DwgReader::from_file(&args.input_file)?;
        reader.read()?
    } else {
        DxfReader::from_file(&args.input_file)?.read()?
    };

    println!(
        "Version: {}, Layers: {}, Entities: {}",
        doc.version.as_str(),
        doc.layers.iter().count(),
        doc.entities().count()
    );

    // 2. 提取所有实体为简化 Feature
    let features = extract_all_features(&doc);
    println!("Extracted {} features", features.len());

    if features.is_empty() {
        println!("No features to process.");
        return Ok(());
    }

    // 3. 计算包围盒
    let data_bbox = compute_bbox(&features);
    println!(
        "Data bbox: ({:.2}, {:.2}) - ({:.2}, {:.2})",
        data_bbox.min_x, data_bbox.min_y, data_bbox.max_x, data_bbox.max_y
    );

    let bbox = args.bbox.unwrap_or(data_bbox);
    println!(
        "Using bbox: ({:.2}, {:.2}) - ({:.2}, {:.2})",
        bbox.min_x, bbox.min_y, bbox.max_x, bbox.max_y
    );

    // 4. 按图层分组 Features
    let mut by_layer: HashMap<String, Vec<SimpleFeature>> = HashMap::new();
    for f in features {
        by_layer.entry(f.layer.clone()).or_default().push(f);
    }

    // 5. 生成瓦片
    let mut total_tiles = 0u64;
    fs::create_dir_all(&args.output_dir)?;

    for zoom in args.min_zoom..=args.max_zoom {
        let (min_tx, min_ty, max_tx, max_ty) = covering_tiles(&bbox, zoom);
        let tile_count = (max_tx - min_tx + 1) as u64 * (max_ty - min_ty + 1) as u64;

        println!(
            "Zoom {}: {}x{} = {} tiles",
            zoom,
            max_tx - min_tx + 1,
            max_ty - min_ty + 1,
            tile_count
        );

        for ty in min_ty..=max_ty {
            for tx in min_tx..=max_tx {
                let tile_bbox = acadrust::mvt::tile_to_bbox(tx, ty, zoom);
                let mut layers = Vec::new();

                for (layer_name, layer_features) in &by_layer {
                    let mut builder = LayerBuilder::new(layer_name.clone(), args.extent);
                    let mut has_features = false;

                    for f in layer_features {
                        match &f.geom {
                            SimpleGeom::Point(x, y) => {
                                if let Some((cx, cy)) = clip_point(*x, *y, &tile_bbox) {
                                    let (tile_x, tile_y) =
                                        world_to_tile(cx, cy, &tile_bbox, args.extent);
                                    builder.add_point(tile_x, tile_y, &f.properties);
                                    has_features = true;
                                }
                            }
                            SimpleGeom::LineString(coords) => {
                                let clipped = clip_linestring(coords, &tile_bbox);
                                for seg in clipped {
                                    let tile_coords: Vec<(i32, i32)> = seg
                                        .iter()
                                        .map(|(x, y)| world_to_tile(*x, *y, &tile_bbox, args.extent))
                                        .collect();
                                    builder.add_linestring(&tile_coords, &f.properties);
                                    has_features = true;
                                }
                            }
                            SimpleGeom::Polygon(rings) => {
                                if let Some(clipped) = clip_polygon(rings, &tile_bbox) {
                                    let tile_rings: Vec<Vec<(i32, i32)>> = clipped
                                        .iter()
                                        .map(|ring| {
                                            ring.iter()
                                                .map(|(x, y)| {
                                                    world_to_tile(*x, *y, &tile_bbox, args.extent)
                                                })
                                                .collect()
                                        })
                                        .collect();
                                    builder.add_polygon(&tile_rings, &f.properties);
                                    has_features = true;
                                }
                            }
                        }
                    }

                    if has_features {
                        layers.push(builder.build());
                    }
                }

                if !layers.is_empty() {
                    let pbf = encode_tile(layers);
                    let tile_dir = format!("{}/{}/{}", args.output_dir, zoom, tx);
                    fs::create_dir_all(&tile_dir)?;
                    let tile_path = format!("{}/{}.pbf", tile_dir, ty);
                    fs::write(&tile_path, &pbf)?;
                    total_tiles += 1;
                }
            }
        }
    }

    println!("Generated {} tiles in {}", total_tiles, args.output_dir);
    Ok(())
}

// ── 辅助函数 ──

/// 将世界坐标转换为瓦片内像素坐标。
fn world_to_tile(wx: f64, wy: f64, tile_bbox: &BBox, extent: u32) -> (i32, i32) {
    let tx = ((wx - tile_bbox.min_x) / tile_bbox.width() * extent as f64).round() as i32;
    let ty = ((tile_bbox.max_y - wy) / tile_bbox.height() * extent as f64).round() as i32;
    (tx, ty)
}

/// 计算所有 Feature 的包围盒。
fn compute_bbox(features: &[SimpleFeature]) -> BBox {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for f in features {
        match &f.geom {
            SimpleGeom::Point(x, y) => {
                min_x = min_x.min(*x);
                min_y = min_y.min(*y);
                max_x = max_x.max(*x);
                max_y = max_y.max(*y);
            }
            SimpleGeom::LineString(coords) => {
                for (x, y) in coords {
                    min_x = min_x.min(*x);
                    min_y = min_y.min(*y);
                    max_x = max_x.max(*x);
                    max_y = max_y.max(*y);
                }
            }
            SimpleGeom::Polygon(rings) => {
                for ring in rings {
                    for (x, y) in ring {
                        min_x = min_x.min(*x);
                        min_y = min_y.min(*y);
                        max_x = max_x.max(*x);
                        max_y = max_y.max(*y);
                    }
                }
            }
        }
    }

    // Handle degenerate bbox (single point or line)
    if min_x == max_x {
        min_x -= 1.0;
        max_x += 1.0;
    }
    if min_y == max_y {
        min_y -= 1.0;
        max_y += 1.0;
    }

    BBox::new(min_x, min_y, max_x, max_y)
}

/// 提取所有实体的简化 Feature。
fn extract_all_features(doc: &CadDocument) -> Vec<SimpleFeature> {
    let mut features = Vec::new();

    for entity in doc.entities() {
        let layer = entity.common().layer.clone();
        match entity {
            EntityType::Line(e) => {
                let color = color_to_string(e.common.color);
                features.push(SimpleFeature {
                    geom: SimpleGeom::LineString(vec![
                        (e.start.x, e.start.y),
                        (e.end.x, e.end.y),
                    ]),
                    layer,
                    properties: vec![("color".into(), color)],
                });
            }
            EntityType::Point(e) => {
                let wcs = ocs_to_wcs(e.normal, e.location);
                let color = color_to_string(e.common.color);
                features.push(SimpleFeature {
                    geom: SimpleGeom::Point(wcs.x, wcs.y),
                    layer,
                    properties: vec![("color".into(), color)],
                });
            }
            EntityType::Circle(e) => {
                let color = color_to_string(e.common.color);
                let center = ocs_to_wcs(e.normal, e.center);
                let mut coords = Vec::new();
                for i in 0..=60 {
                    let angle = i as f64 * 2.0 * std::f64::consts::PI / 60.0;
                    let local = Vector3::new(
                        e.radius * angle.cos(),
                        e.radius * angle.sin(),
                        0.0,
                    );
                    let wcs_pt = center + Matrix3::arbitrary_axis(e.normal) * local;
                    coords.push((wcs_pt.x, wcs_pt.y));
                }
                features.push(SimpleFeature {
                    geom: SimpleGeom::LineString(coords),
                    layer,
                    properties: vec![("color".into(), color)],
                });
            }
            EntityType::Arc(e) => {
                let color = color_to_string(e.common.color);
                let pts = tessellate_arc(e.center, e.radius, e.start_angle, e.end_angle, e.normal);
                let coords: Vec<(f64, f64)> = pts.iter().map(|p| (p.x, p.y)).collect();
                features.push(SimpleFeature {
                    geom: SimpleGeom::LineString(coords),
                    layer,
                    properties: vec![("color".into(), color)],
                });
            }
            EntityType::LwPolyline(e) => {
                let color = color_to_string(e.common.color);
                let coords = lwpolyline_coords(e);
                if coords.len() >= 2 {
                    features.push(SimpleFeature {
                        geom: SimpleGeom::LineString(coords),
                        layer,
                        properties: vec![("color".into(), color)],
                    });
                }
            }
            EntityType::Polyline(e) => {
                let color = color_to_string(e.common.color);
                let coords: Vec<(f64, f64)> =
                    e.vertices.iter().map(|v| (v.location.x, v.location.y)).collect();
                if coords.len() >= 2 {
                    features.push(SimpleFeature {
                        geom: SimpleGeom::LineString(coords),
                        layer,
                        properties: vec![("color".into(), color)],
                    });
                }
            }
            EntityType::Polyline2D(e) => {
                let color = color_to_string(e.common.color);
                let coords = polyline2d_coords(e);
                if coords.len() >= 2 {
                    features.push(SimpleFeature {
                        geom: SimpleGeom::LineString(coords),
                        layer,
                        properties: vec![("color".into(), color)],
                    });
                }
            }
            EntityType::Text(e) => {
                let wcs = ocs_to_wcs(e.normal, e.insertion_point);
                let color = color_to_string(e.common.color);
                features.push(SimpleFeature {
                    geom: SimpleGeom::Point(wcs.x, wcs.y),
                    layer,
                    properties: vec![
                        ("color".into(), color),
                        ("text".into(), e.value.clone()),
                        ("height".into(), format!("{:.2}", e.height)),
                    ],
                });
            }
            EntityType::MText(e) => {
                let color = color_to_string(e.common.color);
                features.push(SimpleFeature {
                    geom: SimpleGeom::Point(e.insertion_point.x, e.insertion_point.y),
                    layer,
                    properties: vec![
                        ("color".into(), color),
                        ("text".into(), e.value.clone()),
                    ],
                });
            }
            EntityType::Ellipse(e) => {
                let color = color_to_string(e.common.color);
                let coords = ellipse_coords(e);
                if coords.len() >= 2 {
                    features.push(SimpleFeature {
                        geom: SimpleGeom::LineString(coords),
                        layer,
                        properties: vec![("color".into(), color)],
                    });
                }
            }
            EntityType::Spline(e) => {
                let color = color_to_string(e.common.color);
                let coords = spline_coords(e);
                if coords.len() >= 2 {
                    features.push(SimpleFeature {
                        geom: SimpleGeom::LineString(coords),
                        layer,
                        properties: vec![("color".into(), color)],
                    });
                }
            }
            EntityType::Solid(e) => {
                let color = color_to_string(e.common.color);
                let ring = vec![
                    (e.first_corner.x, e.first_corner.y),
                    (e.second_corner.x, e.second_corner.y),
                    (e.fourth_corner.x, e.fourth_corner.y),
                    (e.third_corner.x, e.third_corner.y),
                    (e.first_corner.x, e.first_corner.y),
                ];
                features.push(SimpleFeature {
                    geom: SimpleGeom::Polygon(vec![ring]),
                    layer,
                    properties: vec![("color".into(), color)],
                });
            }
            EntityType::Face3D(e) => {
                let color = color_to_string(e.common.color);
                let mut ring = vec![
                    (e.first_corner.x, e.first_corner.y),
                    (e.second_corner.x, e.second_corner.y),
                    (e.third_corner.x, e.third_corner.y),
                ];
                // Fourth corner might be same as third for triangular faces
                if (e.fourth_corner.x - e.third_corner.x).abs() > 1e-9
                    || (e.fourth_corner.y - e.third_corner.y).abs() > 1e-9
                {
                    ring.push((e.fourth_corner.x, e.fourth_corner.y));
                }
                ring.push((e.first_corner.x, e.first_corner.y));
                features.push(SimpleFeature {
                    geom: SimpleGeom::Polygon(vec![ring]),
                    layer,
                    properties: vec![("color".into(), color)],
                });
            }
            _ => {}
        }
    }

    features
}

// ── 几何提取辅助函数 ──

fn lwpolyline_coords(e: &LwPolyline) -> Vec<(f64, f64)> {
    let normal = if e.elevation != 0.0 {
        Vector3::new(0.0, 0.0, 1.0)
    } else {
        e.normal
    };
    let basis = Matrix3::arbitrary_axis(normal);
    let mut coords = Vec::new();

    for i in 0..e.vertices.len() {
        let v = &e.vertices[i];
        let p = basis * Vector3::new(v.location.x, v.location.y, e.elevation);
        coords.push((p.x, p.y));

        if v.bulge.abs() > 1e-10 {
            let next = &e.vertices[(i + 1) % e.vertices.len()];
            let bulge_pts = tessellate_bulge(
                v.location,
                next.location,
                v.bulge,
            );
            // Skip first and last (already added)
            for bp in bulge_pts.iter().skip(1).take(bulge_pts.len().saturating_sub(2)) {
                let wcs = basis * Vector3::new(bp.x, bp.y, e.elevation);
                coords.push((wcs.x, wcs.y));
            }
        }
    }

    if e.is_closed && !coords.is_empty() {
        if coords.first() != coords.last() {
            coords.push(coords[0]);
        }
    }

    coords
}

fn polyline2d_coords(e: &Polyline2D) -> Vec<(f64, f64)> {
    let normal = e.normal;
    let basis = Matrix3::arbitrary_axis(normal);
    let mut coords = Vec::new();

    for i in 0..e.vertices.len() {
        let v = &e.vertices[i];
        let p = basis * Vector3::new(v.location.x, v.location.y, e.elevation);
        coords.push((p.x, p.y));

        if v.bulge.abs() > 1e-10 {
            let next = &e.vertices[(i + 1) % e.vertices.len()];
            let bulge_pts = tessellate_bulge(
                Vector2::new(v.location.x, v.location.y),
                Vector2::new(next.location.x, next.location.y),
                v.bulge,
            );
            for bp in bulge_pts.iter().skip(1).take(bulge_pts.len().saturating_sub(2)) {
                let wcs = basis * Vector3::new(bp.x, bp.y, e.elevation);
                coords.push((wcs.x, wcs.y));
            }
        }
    }

    if e.is_closed() && !coords.is_empty() {
        if coords.first() != coords.last() {
            coords.push(coords[0]);
        }
    }

    coords
}

fn ellipse_coords(e: &Ellipse) -> Vec<(f64, f64)> {
    let mut coords = Vec::new();
    let basis = Matrix3::arbitrary_axis(e.normal);

    // Compute semi-minor axis ratio
    let minor_ratio = e.minor_axis_ratio;

    // Parameter range
    let start = e.start_parameter;
    let mut end = e.end_parameter;
    if end <= start {
        end += 2.0 * std::f64::consts::PI;
    }

    let step = 0.05;
    let n = ((end - start) / step).ceil() as usize;

    for i in 0..=n {
        let t = start + i as f64 * (end - start) / n as f64;
        let cos_t = t.cos();
        let sin_t = t.sin();

        // Major axis endpoint
        let major = e.major_axis;
        // Point on ellipse in OCS
        let local = Vector3::new(
            major.x * cos_t - major.y * sin_t * minor_ratio,
            major.x * sin_t + major.y * cos_t * minor_ratio,
            0.0,
        );
        let wcs = e.center + basis * local;
        coords.push((wcs.x, wcs.y));
    }

    coords
}

fn spline_coords(e: &Spline) -> Vec<(f64, f64)> {
    if e.control_points.is_empty() {
        return Vec::new();
    }

    // Use fit points if available, otherwise evaluate NURBS
    if !e.fit_points.is_empty() {
        return e.fit_points.iter().map(|p| (p.x, p.y)).collect();
    }

    // Simple NURBS evaluation with uniform parameter sampling
    let n = e.control_points.len();
    let degree = e.degree as usize;
    let num_samples = 100;

    let mut coords = Vec::new();
    let knots = &e.knots;

    if knots.len() < n + degree + 1 {
        // Fallback: just use control points
        return e.control_points.iter().map(|p| (p.x, p.y)).collect();
    }

    let u_min = knots[degree];
    let u_max = knots[n];

    for i in 0..=num_samples {
        let u = u_min + (u_max - u_min) * i as f64 / num_samples as f64;
        let pt = nurbs_evaluate(&e.control_points, knots, degree, u);
        coords.push((pt.x, pt.y));
    }

    coords
}

fn nurbs_evaluate(
    control_points: &[Vector3],
    knots: &[f64],
    degree: usize,
    u: f64,
) -> Vector3 {
    let n = control_points.len();
    let _weights = vec![1.0; n]; // Default weights (unused for now)

    // De Boor's algorithm
    let k = find_span(knots, degree, u, n);
    let mut d = vec![Vector3::zero(); degree + 1];

    for j in 0..=degree {
        let idx = (k as i32 - degree as i32 + j as i32) as usize;
        if idx < n {
            d[j] = control_points[idx];
        }
    }

    for r in 1..=degree {
        for j in degree..=degree {
            if j < r {
                continue;
            }
            let idx = (k as i32 - degree as i32 + j as i32) as usize;
            let left = knots.get(idx + r).copied().unwrap_or(0.0);
            let right = knots.get(idx + 1).copied().unwrap_or(0.0);
            let denom = left - right;
            if denom.abs() < 1e-10 {
                continue;
            }
            let alpha = (u - right) / denom;
            d[j] = d[j - 1] * (1.0 - alpha) + d[j] * alpha;
        }
    }

    d[degree]
}

fn find_span(knots: &[f64], degree: usize, u: f64, n: usize) -> usize {
    if u >= knots[n] {
        return n - 1;
    }
    if u <= knots[degree] {
        return degree;
    }

    let mut low = degree;
    let mut high = n;
    let mut mid = (low + high) / 2;

    while u < knots[mid] || u >= knots[mid + 1] {
        if u < knots[mid] {
            high = mid;
        } else {
            low = mid;
        }
        mid = (low + high) / 2;
    }

    mid
}

// ── 弧线和颜色辅助函数 ──

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
        pts.push(Vector3::new(cx + radius * a.cos(), cy + radius * a.sin(), 0.0));
    }
    pts.push(Vector3::new(end.x, end.y, 0.0));
    pts
}

fn ocs_to_wcs(normal: Vector3, point: Vector3) -> Vector3 {
    let basis = Matrix3::arbitrary_axis(normal);
    basis * point
}

fn color_to_string(color: acadrust::types::Color) -> String {
    match color {
        acadrust::types::Color::Rgb { r, g, b } => format!("#{:02x}{:02x}{:02x}", r, g, b),
        acadrust::types::Color::Index(i) => {
            if let Some((r, g, b)) = acadrust::types::Color::Index(i).rgb() {
                format!("#{:02x}{:02x}{:02x}", r, g, b)
            } else {
                "#ffffff".to_string()
            }
        }
        acadrust::types::Color::ByLayer => "#ffffff".to_string(),
        acadrust::types::Color::ByBlock => "#000000".to_string(),
    }
}
