//! MVT (Mapbox Vector Tile) Protobuf encoder
//!
//! Encodes GeoJSON-like features into MVT format using prost.
//! The protobuf message structs are defined directly in Rust
//! using prost derive macros (no protoc dependency required).

use prost::Message;

// ── Protobuf message definitions ──────────────────────────────

/// A single value that can be used as a feature tag.
#[derive(Clone, PartialEq, Message)]
pub struct TileValue {
    #[prost(string, optional, tag = "1")]
    pub string_value: Option<String>,
    #[prost(float, optional, tag = "2")]
    pub float_value: Option<f32>,
    #[prost(double, optional, tag = "3")]
    pub double_value: Option<f64>,
    #[prost(int64, optional, tag = "4")]
    pub int_value: Option<i64>,
    #[prost(uint64, optional, tag = "5")]
    pub uint_value: Option<u64>,
    #[prost(sint64, optional, tag = "6")]
    pub sint_value: Option<i64>,
    #[prost(bool, optional, tag = "7")]
    pub bool_value: Option<bool>,
}

/// Geometry type enumeration for MVT features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum GeomType {
    Unknown = 0,
    Point = 1,
    LineString = 2,
    Polygon = 3,
}

/// A single feature within a tile layer.
#[derive(Clone, PartialEq, Message)]
pub struct TileFeature {
    /// Unique feature ID
    #[prost(uint64, optional, tag = "1")]
    pub id: Option<u64>,
    /// Packed tag indices: [key_idx, value_idx, key_idx, value_idx, ...]
    #[prost(uint32, repeated, tag = "2")]
    pub tags: Vec<u32>,
    /// Geometry type (encoded as i32)
    #[prost(int32, optional, tag = "3", default = "0")]
    pub r#type: Option<i32>,
    /// Encoded geometry commands
    #[prost(uint32, repeated, tag = "4")]
    pub geometry: Vec<u32>,
}

/// A layer within a tile, containing features and shared key/value tables.
#[derive(Clone, PartialEq, Message)]
pub struct TileLayer {
    /// Layer name (typically the CAD layer name)
    #[prost(string, required, tag = "1")]
    pub name: String,
    /// Features in this layer
    #[prost(message, repeated, tag = "2")]
    pub features: Vec<TileFeature>,
    /// Shared key table (attribute names)
    #[prost(string, repeated, tag = "3")]
    pub keys: Vec<String>,
    /// Shared value table (attribute values)
    #[prost(message, repeated, tag = "4")]
    pub values: Vec<TileValue>,
    /// Tile extent in pixels (default 4096)
    #[prost(uint32, optional, tag = "5", default = "4096")]
    pub extent: Option<u32>,
    /// Spec version (must be 2)
    #[prost(uint32, required, tag = "15")]
    pub version: u32,
}

/// The top-level Tile message containing layers.
#[derive(Clone, PartialEq, Message)]
pub struct Tile {
    /// Layers in this tile
    #[prost(message, repeated, tag = "3")]
    pub layers: Vec<TileLayer>,
}

// ── MVT geometry command encoding ─────────────────────────────

/// Encode MVT command integer: (id & 0x7) | (count << 3)
fn command_integer(id: u32, count: u32) -> u32 {
    (id & 0x7) | (count << 3)
}

/// Encode MVT parameter integer using zigzag encoding.
fn parameter_integer(value: i32) -> i32 {
    (value << 1) ^ (value >> 31)
}

// ── Layer builder ─────────────────────────────────────────────

/// Builder for constructing an MVT Layer with deduplicated keys/values.
pub struct LayerBuilder {
    pub name: String,
    pub extent: u32,
    keys: Vec<String>,
    values: Vec<TileValue>,
    key_index: std::collections::HashMap<String, u32>,
    value_index: std::collections::HashMap<String, u32>,
    features: Vec<TileFeature>,
    next_id: u64,
}

impl LayerBuilder {
    pub fn new(name: String, extent: u32) -> Self {
        Self {
            name,
            extent,
            keys: Vec::new(),
            values: Vec::new(),
            key_index: std::collections::HashMap::new(),
            value_index: std::collections::HashMap::new(),
            features: Vec::new(),
            next_id: 1,
        }
    }

    /// Get or insert a key, returning its index.
    fn get_key(&mut self, key: &str) -> u32 {
        if let Some(&idx) = self.key_index.get(key) {
            return idx;
        }
        let idx = self.keys.len() as u32;
        self.keys.push(key.to_string());
        self.key_index.insert(key.to_string(), idx);
        idx
    }

    /// Get or insert a value, returning its index.
    fn get_value(&mut self, val: &TileValue) -> u32 {
        // Use the string representation as the hash key for dedup
        let key = format!("{:?}", val);
        if let Some(&idx) = self.value_index.get(&key) {
            return idx;
        }
        let idx = self.values.len() as u32;
        self.values.push(val.clone());
        self.value_index.insert(key, idx);
        idx
    }

    /// Add a point feature with properties.
    pub fn add_point(&mut self, x: i32, y: i32, props: &[(String, String)]) {
        let mut tags = Vec::new();
        for (k, v) in props {
            let ki = self.get_key(k);
            let vi = self.get_value(&TileValue { string_value: Some(v.clone()), ..Default::default() });
            tags.push(ki);
            tags.push(vi);
        }
        let geom = vec![
            command_integer(1, 1), // MoveTo, count=1
            parameter_integer(x) as u32,
            parameter_integer(y) as u32,
        ];
        self.features.push(TileFeature {
            id: Some(self.next_id),
            tags,
            r#type: Some(GeomType::Point as i32),
            geometry: geom,
        });
        self.next_id += 1;
    }

    /// Add a linestring feature.
    pub fn add_linestring(&mut self, coords: &[(i32, i32)], props: &[(String, String)]) {
        if coords.len() < 2 {
            return;
        }
        let mut tags = Vec::new();
        for (k, v) in props {
            let ki = self.get_key(k);
            let vi = self.get_value(&TileValue { string_value: Some(v.clone()), ..Default::default() });
            tags.push(ki);
            tags.push(vi);
        }
        let mut geom = Vec::new();
        // MoveTo first point
        geom.push(command_integer(1, 1));
        geom.push(parameter_integer(coords[0].0) as u32);
        geom.push(parameter_integer(coords[0].1) as u32);
        // LineTo remaining points
        let count = (coords.len() - 1) as u32;
        geom.push(command_integer(2, count));
        let mut prev = coords[0];
        for &c in &coords[1..] {
            geom.push(parameter_integer(c.0 - prev.0) as u32);
            geom.push(parameter_integer(c.1 - prev.1) as u32);
            prev = c;
        }
        self.features.push(TileFeature {
            id: Some(self.next_id),
            tags,
            r#type: Some(GeomType::LineString as i32),
            geometry: geom,
        });
        self.next_id += 1;
    }

    /// Add a polygon feature.
    pub fn add_polygon(&mut self, rings: &[Vec<(i32, i32)>], props: &[(String, String)]) {
        if rings.is_empty() || rings[0].len() < 4 {
            return;
        }
        let mut tags = Vec::new();
        for (k, v) in props {
            let ki = self.get_key(k);
            let vi = self.get_value(&TileValue { string_value: Some(v.clone()), ..Default::default() });
            tags.push(ki);
            tags.push(vi);
        }
        let mut geom = Vec::new();
        for ring in rings {
            if ring.len() < 4 {
                continue;
            }
            // MoveTo first point
            geom.push(command_integer(1, 1));
            geom.push(parameter_integer(ring[0].0) as u32);
            geom.push(parameter_integer(ring[0].1) as u32);
            // LineTo remaining points (skip last if it equals first)
            let end = if ring.last() == ring.first() {
                ring.len() - 1
            } else {
                ring.len()
            };
            let count = (end - 1) as u32;
            geom.push(command_integer(2, count));
            let mut prev = ring[0];
            for &c in &ring[1..end] {
                geom.push(parameter_integer(c.0 - prev.0) as u32);
                geom.push(parameter_integer(c.1 - prev.1) as u32);
                prev = c;
            }
            // ClosePath
            geom.push(command_integer(7, 1));
        }
        self.features.push(TileFeature {
            id: Some(self.next_id),
            tags,
            r#type: Some(GeomType::Polygon as i32),
            geometry: geom,
        });
        self.next_id += 1;
    }

    /// Build the final TileLayer.
    pub fn build(self) -> TileLayer {
        TileLayer {
            name: self.name,
            features: self.features,
            keys: self.keys,
            values: self.values,
            extent: Some(self.extent),
            version: 2,
        }
    }
}

/// Encode a tile (with layers) into protobuf bytes.
pub fn encode_tile(layers: Vec<TileLayer>) -> Vec<u8> {
    let tile = Tile { layers };
    tile.encode_to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_feature_encode_decode() {
        let mut builder = LayerBuilder::new("test_layer".into(), 4096);
        builder.add_point(100, 200, &[("name".into(), "point1".into())]);
        let layer = builder.build();
        let pbf = encode_tile(vec![layer]);

        // Decode and verify
        let decoded = Tile::decode(pbf.as_slice()).expect("decode failed");
        assert_eq!(decoded.layers.len(), 1);
        let layer = &decoded.layers[0];
        assert_eq!(layer.name, "test_layer");
        assert_eq!(layer.version, 2);
        assert_eq!(layer.extent, Some(4096));
        assert_eq!(layer.features.len(), 1);
        assert_eq!(layer.features[0].r#type, Some(GeomType::Point as i32));
        assert_eq!(layer.keys, vec!["name"]);
        assert_eq!(
            layer.values[0].string_value,
            Some("point1".to_string())
        );
    }

    #[test]
    fn test_linestring_feature_encode_decode() {
        let mut builder = LayerBuilder::new("lines".into(), 4096);
        builder.add_linestring(
            &[(0, 0), (100, 0), (100, 100)],
            &[("color".into(), "#ff0000".into())],
        );
        let layer = builder.build();
        let pbf = encode_tile(vec![layer]);

        let decoded = Tile::decode(pbf.as_slice()).expect("decode failed");
        assert_eq!(decoded.layers.len(), 1);
        let f = &decoded.layers[0].features[0];
        assert_eq!(f.r#type, Some(GeomType::LineString as i32));
        // Geometry should have: MoveTo(1) + LineTo(2)
        assert!(!f.geometry.is_empty());
    }

    #[test]
    fn test_polygon_feature_encode_decode() {
        let mut builder = LayerBuilder::new("polys".into(), 4096);
        builder.add_polygon(
            &[vec![(0, 0), (100, 0), (100, 100), (0, 100), (0, 0)]],
            &[("area".into(), "10000".into())],
        );
        let layer = builder.build();
        let pbf = encode_tile(vec![layer]);

        let decoded = Tile::decode(pbf.as_slice()).expect("decode failed");
        let f = &decoded.layers[0].features[0];
        assert_eq!(f.r#type, Some(GeomType::Polygon as i32));
    }

    #[test]
    fn test_multi_layer_encode_decode() {
        let mut b1 = LayerBuilder::new("layer_a".into(), 4096);
        b1.add_point(10, 20, &[]);
        let mut b2 = LayerBuilder::new("layer_b".into(), 4096);
        b2.add_linestring(&[(0, 0), (50, 50)], &[]);
        let pbf = encode_tile(vec![b1.build(), b2.build()]);

        let decoded = Tile::decode(pbf.as_slice()).expect("decode failed");
        assert_eq!(decoded.layers.len(), 2);
        assert_eq!(decoded.layers[0].name, "layer_a");
        assert_eq!(decoded.layers[1].name, "layer_b");
    }

    #[test]
    fn test_key_value_deduplication() {
        let mut builder = LayerBuilder::new("dedup".into(), 4096);
        builder.add_point(1, 1, &[("color".into(), "red".into())]);
        builder.add_point(2, 2, &[("color".into(), "red".into())]);
        builder.add_point(3, 3, &[("color".into(), "blue".into())]);
        let layer = builder.build();

        // Keys should be deduplicated: ["color"]
        assert_eq!(layer.keys, vec!["color"]);
        // Values should be deduplicated: ["red", "blue"]
        assert_eq!(layer.values.len(), 2);
        // Each feature should have 2 tags (key_idx + value_idx)
        assert_eq!(layer.features[0].tags.len(), 2);
        assert_eq!(layer.features[1].tags.len(), 2);
        // First two features should share the same key and value indices
        assert_eq!(layer.features[0].tags, layer.features[1].tags);
    }
}
