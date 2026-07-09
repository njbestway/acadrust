//! MVT (Mapbox Vector Tile) generation module
//!
//! Provides functionality to convert CAD entities into
//! Mapbox Vector Tiles for web map serving.

pub mod clipper;
pub mod encoder;
pub mod tile;

pub use clipper::{clip_linestring, clip_point, clip_polygon, SimpleGeom};
pub use encoder::{encode_tile, LayerBuilder};
pub use tile::{covering_tiles, geographic_to_mercator_bbox, mercator_project, mercator_to_tile, tile_to_bbox, BBox};
