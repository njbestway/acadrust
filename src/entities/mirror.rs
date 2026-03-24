//! Centralized mirror implementations for all entity types.
//!
//! Entities that need post-processing after mirroring (arc angle swaps,
//! bulge negation, face-winding reversal, etc.) have custom implementations
//! here.  Entities without mirror-specific corrections simply delegate to
//! [`apply_transform`](super::transform).

use super::*;
use crate::types::{Transform, Vector3};

// ── Arc ──────────────────────────────────────────────────────────────────────

pub(crate) fn mirror_arc(e: &mut Arc, transform: &Transform) {
    let start_pt = e.start_point();
    let end_pt = e.end_point();

    super::transform::transform_arc(e, transform);

    let mirrored_start = transform.apply(end_pt);
    let mirrored_end = transform.apply(start_pt);

    e.start_angle = crate::types::normalize_angle(
        (mirrored_start.y - e.center.y).atan2(mirrored_start.x - e.center.x),
    );
    e.end_angle = crate::types::normalize_angle(
        (mirrored_end.y - e.center.y).atan2(mirrored_end.x - e.center.x),
    );
}

// ── Ellipse ──────────────────────────────────────────────────────────────────

pub(crate) fn mirror_ellipse(e: &mut Ellipse, transform: &Transform) {
    super::transform::transform_ellipse(e, transform);

    if !e.is_full() {
        let new_start = -e.end_parameter;
        let new_end = -e.start_parameter;
        e.start_parameter = new_start;
        e.end_parameter = new_end;
    }
}

// ── LwPolyline ───────────────────────────────────────────────────────────────

pub(crate) fn mirror_lwpolyline(e: &mut LwPolyline, transform: &Transform) {
    super::transform::transform_lwpolyline(e, transform);
    for vertex in &mut e.vertices {
        vertex.bulge = -vertex.bulge;
    }
}

// ── Polyline (3D heavy) ──────────────────────────────────────────────────────

pub(crate) fn mirror_polyline2d(e: &mut Polyline2D, transform: &Transform) {
    super::transform::transform_polyline2d(e, transform);
    for vertex in &mut e.vertices {
        vertex.bulge = -vertex.bulge;
    }
}

// ── Face3D ───────────────────────────────────────────────────────────────────

pub(crate) fn mirror_face3d(e: &mut Face3D, transform: &Transform) {
    super::transform::transform_face3d(e, transform);
    std::mem::swap(&mut e.second_corner, &mut e.fourth_corner);
}

// ── Solid ────────────────────────────────────────────────────────────────────

pub(crate) fn mirror_solid(e: &mut Solid, transform: &Transform) {
    super::transform::transform_solid(e, transform);
    std::mem::swap(&mut e.second_corner, &mut e.fourth_corner);
}

// ── Mesh ─────────────────────────────────────────────────────────────────────

pub(crate) fn mirror_mesh(e: &mut Mesh, transform: &Transform) {
    super::transform::transform_mesh(e, transform);
    for face in &mut e.faces {
        face.reverse();
    }
}

// ── PolyfaceMesh ─────────────────────────────────────────────────────────────

pub(crate) fn mirror_polyface_mesh(e: &mut PolyfaceMesh, transform: &Transform) {
    super::transform::transform_polyface_mesh(e, transform);
    for face in &mut e.faces {
        face.reverse();
    }
}

// ── Text ─────────────────────────────────────────────────────────────────────

pub(crate) fn mirror_text(e: &mut Text, transform: &Transform) {
    super::transform::transform_text(e, transform);
    let dir = Vector3::new(e.rotation.cos(), e.rotation.sin(), 0.0);
    let mirrored_dir = transform.apply_rotation(dir);
    e.rotation = mirrored_dir.y.atan2(mirrored_dir.x);
    e.oblique_angle = -e.oblique_angle;
}

// ── MText ────────────────────────────────────────────────────────────────────

pub(crate) fn mirror_mtext(e: &mut MText, transform: &Transform) {
    super::transform::transform_mtext(e, transform);
    let dir = Vector3::new(e.rotation.cos(), e.rotation.sin(), 0.0);
    let mirrored_dir = transform.apply_rotation(dir);
    e.rotation = mirrored_dir.y.atan2(mirrored_dir.x);
}

// ── Shape ────────────────────────────────────────────────────────────────────

pub(crate) fn mirror_shape(e: &mut Shape, transform: &Transform) {
    super::transform::transform_shape(e, transform);
    let dir = Vector3::new(e.rotation.cos(), e.rotation.sin(), 0.0);
    let mirrored_dir = transform.apply_rotation(dir);
    e.rotation = mirrored_dir.y.atan2(mirrored_dir.x);
    e.relative_x_scale = -e.relative_x_scale;
    e.oblique_angle = -e.oblique_angle;
}

// ── Hatch ────────────────────────────────────────────────────────────────────

pub(crate) fn mirror_hatch(e: &mut Hatch, transform: &Transform) {
    use crate::types::normalize_angle;

    super::transform::transform_hatch(e, transform);

    for path in &mut e.paths {
        for edge in &mut path.edges {
            match edge {
                BoundaryEdge::CircularArc(arc) => {
                    let old_start = arc.start_angle;
                    let old_end = arc.end_angle;

                    let center_3d = Vector3::new(arc.center.x, arc.center.y, 0.0);
                    let start_pt = Vector3::new(
                        arc.center.x + arc.radius * old_start.cos(),
                        arc.center.y + arc.radius * old_start.sin(),
                        0.0,
                    );
                    let end_pt = Vector3::new(
                        arc.center.x + arc.radius * old_end.cos(),
                        arc.center.y + arc.radius * old_end.sin(),
                        0.0,
                    );
                    let ms = transform.apply(start_pt);
                    let me = transform.apply(end_pt);

                    arc.start_angle =
                        normalize_angle((me.y - center_3d.y).atan2(me.x - center_3d.x));
                    arc.end_angle =
                        normalize_angle((ms.y - center_3d.y).atan2(ms.x - center_3d.x));
                    arc.counter_clockwise = !arc.counter_clockwise;
                }
                BoundaryEdge::EllipticArc(ellipse) => {
                    let new_start = -ellipse.end_angle;
                    let new_end = -ellipse.start_angle;
                    ellipse.start_angle = new_start;
                    ellipse.end_angle = new_end;
                    ellipse.counter_clockwise = !ellipse.counter_clockwise;
                }
                BoundaryEdge::Polyline(poly) => {
                    for v in &mut poly.vertices {
                        v.z = -v.z;
                    }
                }
                _ => {}
            }
        }
    }
}

// ── EntityType dispatch ──────────────────────────────────────────────────────

impl EntityType {
    /// Apply a mirror transform with entity-specific corrections.
    ///
    /// Entities that require post-processing (arc angle swaps, bulge
    /// negation, face winding reversal) are handled here.  All other
    /// entities simply delegate to [`apply_transform`](EntityType::apply_transform).
    pub fn apply_mirror(&mut self, transform: &Transform) {
        match self {
            // Entities with custom mirror behaviour
            EntityType::Arc(e) => mirror_arc(e, transform),
            EntityType::Ellipse(e) => mirror_ellipse(e, transform),
            EntityType::LwPolyline(e) => mirror_lwpolyline(e, transform),
            EntityType::Polyline2D(e) => mirror_polyline2d(e, transform),
            EntityType::Face3D(e) => mirror_face3d(e, transform),
            EntityType::Solid(e) => mirror_solid(e, transform),
            EntityType::Mesh(e) => mirror_mesh(e, transform),
            EntityType::PolyfaceMesh(e) => mirror_polyface_mesh(e, transform),
            EntityType::Text(e) => mirror_text(e, transform),
            EntityType::MText(e) => mirror_mtext(e, transform),
            EntityType::Shape(e) => mirror_shape(e, transform),
            EntityType::Hatch(e) => mirror_hatch(e, transform),

            // All other entities: mirror = apply_transform (no corrections needed)
            _ => self.apply_transform(transform),
        }
    }

    /// Mirror across the YZ plane (negate X coordinates).
    pub fn mirror_x(&mut self) {
        self.apply_mirror(&Transform::from_mirror_x());
    }

    /// Mirror across the XZ plane (negate Y coordinates).
    pub fn mirror_y(&mut self) {
        self.apply_mirror(&Transform::from_mirror_y());
    }

    /// Mirror across the XY plane (negate Z coordinates).
    pub fn mirror_z(&mut self) {
        self.apply_mirror(&Transform::from_mirror_z());
    }

    /// Mirror across a line defined by two points (in the XY plane).
    pub fn mirror_about_line(&mut self, p1: Vector3, p2: Vector3) {
        self.apply_mirror(&Transform::from_mirror_line(p1, p2));
    }

    /// Mirror across an arbitrary plane.
    pub fn mirror_about_plane(&mut self, point: Vector3, normal: Vector3) {
        self.apply_mirror(&Transform::from_mirror_plane(point, normal));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Vector3;

    #[test]
    fn test_mirror_x_line() {
        let mut entity = EntityType::Line(Line::from_points(
            Vector3::new(1.0, 2.0, 0.0),
            Vector3::new(3.0, 4.0, 0.0),
        ));
        entity.mirror_x();
        if let EntityType::Line(line) = &entity {
            assert!((line.start.x - (-1.0)).abs() < 1e-10);
            assert!((line.start.y - 2.0).abs() < 1e-10);
            assert!((line.end.x - (-3.0)).abs() < 1e-10);
            assert!((line.end.y - 4.0).abs() < 1e-10);
        } else {
            panic!("Expected Line");
        }
    }

    #[test]
    fn test_mirror_solid_swaps_winding() {
        let mut solid = Solid::new(
            Vector3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 1.0, 0.0),
        );
        let original_second = solid.second_corner;
        let original_fourth = solid.fourth_corner;
        mirror_solid(&mut solid, &Transform::from_mirror_x());
        assert_ne!(solid.second_corner, original_second);
        assert_ne!(solid.fourth_corner, original_fourth);
    }

    #[test]
    fn test_mirror_lwpolyline_negates_bulge() {
        let mut lw = LwPolyline::new();
        lw.vertices.push(LwVertex {
            location: crate::types::Vector2::new(0.0, 0.0),
            bulge: 0.5,
            start_width: 0.0,
            end_width: 0.0,
        });
        lw.vertices.push(LwVertex {
            location: crate::types::Vector2::new(10.0, 0.0),
            bulge: -0.3,
            start_width: 0.0,
            end_width: 0.0,
        });
        mirror_lwpolyline(&mut lw, &Transform::from_mirror_x());
        assert!((lw.vertices[0].bulge - (-0.5)).abs() < 1e-10);
        assert!((lw.vertices[1].bulge - 0.3).abs() < 1e-10);
    }
}
