//! Feature clipping to tile boundaries
//!
//! Clips GeoJSON-like features to a bounding box for tile generation.

use super::tile::BBox;

/// A simplified geometry representation for clipping.
#[derive(Debug, Clone)]
pub enum SimpleGeom {
    Point(f64, f64),
    LineString(Vec<(f64, f64)>),
    Polygon(Vec<Vec<(f64, f64)>>),
    MultiPolygon(Vec<Vec<Vec<(f64, f64)>>>),
}

/// Clip a point to a bbox. Returns None if outside.
pub fn clip_point(x: f64, y: f64, bbox: &BBox) -> Option<(f64, f64)> {
    if bbox.contains(x, y) {
        Some((x, y))
    } else {
        None
    }
}

/// Clip a linestring to a bbox using Cohen-Sutherland algorithm.
/// Returns a list of clipped line segments (may be empty or multiple).
pub fn clip_linestring(coords: &[(f64, f64)], bbox: &BBox) -> Vec<Vec<(f64, f64)>> {
    let mut result = Vec::new();
    let mut current_segment = Vec::new();

    for window in coords.windows(2) {
        let p1 = window[0];
        let p2 = window[1];
        if let Some(clipped) = clip_segment_cohen_sutherland(p1, p2, bbox) {
            if current_segment.is_empty() {
                current_segment.push(clipped.0);
            } else if (current_segment.last().unwrap().0 - clipped.0 .0).abs() > 1e-10
                || (current_segment.last().unwrap().1 - clipped.0 .1).abs() > 1e-10
            {
                // Discontinuity: start a new segment
                if current_segment.len() >= 2 {
                    result.push(std::mem::take(&mut current_segment));
                }
                current_segment.clear();
                current_segment.push(clipped.0);
            }
            current_segment.push(clipped.1);
        } else {
            // Segment fully outside: flush current segment
            if current_segment.len() >= 2 {
                result.push(std::mem::take(&mut current_segment));
            }
            current_segment.clear();
        }
    }

    if current_segment.len() >= 2 {
        result.push(current_segment);
    }

    result
}

/// Clip a polygon to a bbox using Sutherland-Hodgman algorithm.
/// Returns the clipped polygon rings (may be empty).
pub fn clip_polygon(rings: &[Vec<(f64, f64)>], bbox: &BBox) -> Option<Vec<Vec<(f64, f64)>>> {
    if rings.is_empty() {
        return None;
    }

    let mut result_rings = Vec::new();

    for ring in rings {
        let clipped = sutherland_hodgman(ring, bbox);
        if clipped.len() >= 3 {
            // Close the ring
            let mut closed = clipped;
            if closed.first() != closed.last() {
                closed.push(closed[0]);
            }
            result_rings.push(closed);
        }
    }

    if result_rings.is_empty() {
        None
    } else {
        Some(result_rings)
    }
}

// ── Cohen-Sutherland line clipping ────────────────────────────

const INSIDE: u8 = 0;
const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const BOTTOM: u8 = 4;
const TOP: u8 = 8;

fn outcode(x: f64, y: f64, bbox: &BBox) -> u8 {
    let mut code = INSIDE;
    if x < bbox.min_x {
        code |= LEFT;
    }
    if x > bbox.max_x {
        code |= RIGHT;
    }
    if y < bbox.min_y {
        code |= BOTTOM;
    }
    if y > bbox.max_y {
        code |= TOP;
    }
    code
}

/// Clip a single line segment to a bbox. Returns clipped endpoints or None.
fn clip_segment_cohen_sutherland(
    mut p1: (f64, f64),
    mut p2: (f64, f64),
    bbox: &BBox,
) -> Option<((f64, f64), (f64, f64))> {
    let mut code1 = outcode(p1.0, p1.1, bbox);
    let mut code2 = outcode(p2.0, p2.1, bbox);

    loop {
        if (code1 | code2) == 0 {
            // Both inside
            return Some((p1, p2));
        }
        if (code1 & code2) != 0 {
            // Both in same outside zone
            return None;
        }

        let code_out = if code1 != 0 { code1 } else { code2 };
        let (x, y);

        if code_out & TOP != 0 {
            x = p1.0 + (p2.0 - p1.0) * (bbox.max_y - p1.1) / (p2.1 - p1.1);
            y = bbox.max_y;
        } else if code_out & BOTTOM != 0 {
            x = p1.0 + (p2.0 - p1.0) * (bbox.min_y - p1.1) / (p2.1 - p1.1);
            y = bbox.min_y;
        } else if code_out & RIGHT != 0 {
            y = p1.1 + (p2.1 - p1.1) * (bbox.max_x - p1.0) / (p2.0 - p1.0);
            x = bbox.max_x;
        } else {
            // LEFT
            y = p1.1 + (p2.1 - p1.1) * (bbox.min_x - p1.0) / (p2.0 - p1.0);
            x = bbox.min_x;
        }

        if code_out == code1 {
            p1 = (x, y);
            code1 = outcode(x, y, bbox);
        } else {
            p2 = (x, y);
            code2 = outcode(x, y, bbox);
        }
    }
}

// ── Sutherland-Hodgman polygon clipping ───────────────────────

/// Clip a polygon (as a list of vertices) against a bbox.
fn sutherland_hodgman(polygon: &[(f64, f64)], bbox: &BBox) -> Vec<(f64, f64)> {
    if polygon.is_empty() {
        return Vec::new();
    }

    // Clip against each edge of the bbox
    let mut output: Vec<(f64, f64)> = polygon.to_vec();

    // Remove closing vertex if present
    if output.len() >= 2 && output.first() == output.last() {
        output.pop();
    }

    // Clip against left edge (x = min_x)
    output = clip_polygon_edge(&output, |p| p.0 >= bbox.min_x, |a, b| {
        let t = (bbox.min_x - a.0) / (b.0 - a.0);
        (bbox.min_x, a.1 + t * (b.1 - a.1))
    });

    // Clip against right edge (x = max_x)
    output = clip_polygon_edge(&output, |p| p.0 <= bbox.max_x, |a, b| {
        let t = (bbox.max_x - a.0) / (b.0 - a.0);
        (bbox.max_x, a.1 + t * (b.1 - a.1))
    });

    // Clip against bottom edge (y = min_y)
    output = clip_polygon_edge(&output, |p| p.1 >= bbox.min_y, |a, b| {
        let t = (bbox.min_y - a.1) / (b.1 - a.1);
        (a.0 + t * (b.0 - a.0), bbox.min_y)
    });

    // Clip against top edge (y = max_y)
    output = clip_polygon_edge(&output, |p| p.1 <= bbox.max_y, |a, b| {
        let t = (bbox.max_y - a.1) / (b.1 - a.1);
        (a.0 + t * (b.0 - a.0), bbox.max_y)
    });

    output
}

/// Clip polygon vertices against a single edge.
fn clip_polygon_edge(
    vertices: &[(f64, f64)],
    inside: impl Fn(&(f64, f64)) -> bool,
    intersect: impl Fn((f64, f64), (f64, f64)) -> (f64, f64),
) -> Vec<(f64, f64)> {
    if vertices.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let n = vertices.len();

    for i in 0..n {
        let current = vertices[i];
        let next = vertices[(i + 1) % n];

        if inside(&current) {
            result.push(current);
            if !inside(&next) {
                result.push(intersect(current, next));
            }
        } else if inside(&next) {
            result.push(intersect(current, next));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clip_point_inside() {
        let bbox = BBox::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(clip_point(50.0, 50.0, &bbox), Some((50.0, 50.0)));
    }

    #[test]
    fn test_clip_point_outside() {
        let bbox = BBox::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(clip_point(150.0, 50.0, &bbox), None);
    }

    #[test]
    fn test_clip_linestring_full_inside() {
        let bbox = BBox::new(0.0, 0.0, 100.0, 100.0);
        let coords = vec![(10.0, 10.0), (50.0, 50.0), (90.0, 10.0)];
        let result = clip_linestring(&coords, &bbox);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].len(), 3);
    }

    #[test]
    fn test_clip_linestring_full_outside() {
        let bbox = BBox::new(0.0, 0.0, 100.0, 100.0);
        let coords = vec![(150.0, 10.0), (150.0, 90.0)];
        let result = clip_linestring(&coords, &bbox);
        assert!(result.is_empty());
    }

    #[test]
    fn test_clip_polygon_square() {
        let bbox = BBox::new(0.0, 0.0, 50.0, 50.0);
        let ring = vec![(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0), (0.0, 0.0)];
        let result = clip_polygon(&[ring], &bbox);
        assert!(result.is_some());
        let clipped = result.unwrap();
        assert_eq!(clipped.len(), 1);
        assert!(clipped[0].len() >= 4); // At least a square + closing point
    }
}
