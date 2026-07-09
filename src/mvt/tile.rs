//! Tile coordinate calculations
//!
//! Converts between geographic coordinates (lon/lat), Web Mercator,
//! and tile coordinates (z/x/y).

use std::f64::consts::PI;

/// Earth circumference in meters (Web Mercator)
const EARTH_CIRCUMFERENCE: f64 = 20_037_508.342789244;

/// A bounding box in Mercator coordinates (meters).
#[derive(Debug, Clone, Copy)]
pub struct BBox {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BBox {
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self { min_x, min_y, max_x, max_y }
    }

    /// Check if a point is within the bbox.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// Check if this bbox intersects another.
    pub fn intersects(&self, other: &BBox) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }

    /// Width of the bbox.
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    /// Height of the bbox.
    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }
}

/// Project WGS84 (lon, lat) to Web Mercator (x, y) in meters.
pub fn mercator_project(lon: f64, lat: f64) -> (f64, f64) {
    let x = lon * EARTH_CIRCUMFERENCE / 180.0;
    let y = ((90.0 + lat) * PI / 360.0).tan().ln() * EARTH_CIRCUMFERENCE / PI;
    (x, y)
}

/// Inverse Mercator projection: (x, y) meters -> (lon, lat).
pub fn mercator_unproject(x: f64, y: f64) -> (f64, f64) {
    let lon = x * 180.0 / EARTH_CIRCUMFERENCE;
    let lat = (y * PI / EARTH_CIRCUMFERENCE).exp().atan() * 360.0 / PI - 90.0;
    (lon, lat)
}

/// Convert longitude to tile X coordinate at a given zoom level.
pub fn lon_to_tile_x(lon: f64, zoom: u32) -> u32 {
    let n = 2.0_f64.powi(zoom as i32);
    ((lon + 180.0) / 360.0 * n).floor() as u32
}

/// Convert latitude to tile Y coordinate at a given zoom level.
pub fn lat_to_tile_y(lat: f64, zoom: u32) -> u32 {
    let n = 2.0_f64.powi(zoom as i32);
    let lat_rad = lat.to_radians();
    ((1.0 - lat_rad.tan().asinh() / PI) / 2.0 * n).floor() as u32
}

/// Get the Mercator bounding box for a tile.
pub fn tile_to_bbox(tx: u32, ty: u32, zoom: u32) -> BBox {
    let n = 2.0_f64.powi(zoom as i32);
    let min_x = tx as f64 / n * 2.0 * EARTH_CIRCUMFERENCE - EARTH_CIRCUMFERENCE;
    let max_x = (tx + 1) as f64 / n * 2.0 * EARTH_CIRCUMFERENCE - EARTH_CIRCUMFERENCE;
    let min_y = EARTH_CIRCUMFERENCE - (ty + 1) as f64 / n * 2.0 * EARTH_CIRCUMFERENCE;
    let max_y = EARTH_CIRCUMFERENCE - ty as f64 / n * 2.0 * EARTH_CIRCUMFERENCE;
    BBox::new(min_x, min_y, max_x, max_y)
}

/// Get the range of tiles that cover a given Mercator bbox at a zoom level.
/// Returns (min_x, min_y, max_x, max_y) tile coordinates.
pub fn covering_tiles(bbox: &BBox, zoom: u32) -> (u32, u32, u32, u32) {
    let n = 2.0_f64.powi(zoom as i32);
    let min_tx = ((bbox.min_x + EARTH_CIRCUMFERENCE) / (2.0 * EARTH_CIRCUMFERENCE) * n)
        .floor()
        .max(0.0) as u32;
    let max_tx = ((bbox.max_x + EARTH_CIRCUMFERENCE) / (2.0 * EARTH_CIRCUMFERENCE) * n)
        .floor()
        .min(n as f64 - 1.0)
        .max(0.0) as u32;
    let min_ty = ((EARTH_CIRCUMFERENCE - bbox.max_y) / (2.0 * EARTH_CIRCUMFERENCE) * n)
        .floor()
        .max(0.0) as u32;
    let max_ty = ((EARTH_CIRCUMFERENCE - bbox.min_y) / (2.0 * EARTH_CIRCUMFERENCE) * n)
        .floor()
        .min(n as f64 - 1.0)
        .max(0.0) as u32;
    (min_tx, min_ty, max_tx, max_ty)
}

/// Convert a Mercator coordinate to tile-local pixel coordinates (0..extent).
pub fn mercator_to_tile(
    mx: f64,
    my: f64,
    tile_bbox: &BBox,
    extent: u32,
) -> (i32, i32) {
    let tx = ((mx - tile_bbox.min_x) / tile_bbox.width() * extent as f64).round() as i32;
    let ty = ((tile_bbox.max_y - my) / tile_bbox.height() * extent as f64).round() as i32;
    (tx, ty)
}

/// Convert a geographic bbox (lon/lat) to Mercator bbox.
pub fn geographic_to_mercator_bbox(west: f64, south: f64, east: f64, north: f64) -> BBox {
    let (min_x, max_y) = mercator_project(west, north);
    let (max_x, min_y) = mercator_project(east, south);
    BBox::new(min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mercator_roundtrip() {
        let (lon, lat) = (116.3975, 39.9085); // Beijing
        let (mx, my) = mercator_project(lon, lat);
        let (lon2, lat2) = mercator_unproject(mx, my);
        assert!((lon - lon2).abs() < 1e-6);
        assert!((lat - lat2).abs() < 1e-6);
    }

    #[test]
    fn test_tile_bbox() {
        let bbox = tile_to_bbox(0, 0, 0);
        assert!((bbox.min_x + EARTH_CIRCUMFERENCE).abs() < 1.0);
        assert!((bbox.max_y - EARTH_CIRCUMFERENCE).abs() < 1.0);
    }

    #[test]
    fn test_mercator_to_tile() {
        // Test center of the world tile at zoom 0
        let bbox = tile_to_bbox(0, 0, 0);
        let (tx, ty) = mercator_to_tile(0.0, 0.0, &bbox, 4096);
        assert_eq!(tx, 2048);
        assert_eq!(ty, 2048);
    }
}
