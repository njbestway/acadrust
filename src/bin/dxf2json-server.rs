//! dxf2json-server — HTTP API 服务，接受 DWG/DXF 文件上传，返回 GeoJSON
//!
//! # 启动
//! ```sh
//! cargo run --bin dxf2json-server --features server
//! cargo run --bin dxf2json-server --features server -- --port 8080
//! ```
//!
//! # API
//! ```
//! POST /convert
//! Content-Type: multipart/form-data
//! Body: file (DWG/DXF 文件)
//!
//! Response: { "layers": [{ "name": "图层1", "geojson": {...} }, ...] }
//! ```

// Include the shared conversion logic from dxf2json.rs.
// When `server` feature is enabled, CLI-only items (Cli, main, process_file) are
// excluded via #[cfg(not(feature = "server"))], so only the conversion functions remain.
#[path = "dxf2json.rs"]
mod dxf2json_convert;

use axum::{
    extract::{DefaultBodyLimit, Multipart},
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use std::io::Cursor;
use std::net::SocketAddr;

/// 最大上传文件 200MB
const MAX_UPLOAD_SIZE: usize = 200 * 1024 * 1024;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .skip_while(|a| a != "--port")
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let app = Router::new()
        .route("/health", get(health_check))
        .route("/convert", post(convert_file))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_SIZE));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("dxf2json-server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn health_check() -> &'static str {
    "OK"
}

async fn convert_file(mut multipart: Multipart) -> Result<Json<Value>, (StatusCode, String)> {
    // 1. 读取上传的文件
    let mut file_data: Option<(String, Vec<u8>)> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("multipart error: {}", e)))?
    {
        let name = field.name().unwrap_or("file").to_string();
        if name == "file" {
            let filename = field
                .file_name()
                .unwrap_or("upload.dwg")
                .to_string();
            let data = field
                .bytes()
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, format!("read error: {}", e)))?;
            file_data = Some((filename, data.to_vec()));
        }
    }

    let (filename, data) = file_data
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "no 'file' field in multipart".to_string()))?;

    // 2. 解析 DWG/DXF
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("dxf")
        .to_lowercase();

    let doc = if ext == "dwg" {
        acadrust::DwgReader::from_stream(Cursor::new(data))
            .read()
            .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, format!("DWG parse error: {}", e)))?
    } else {
        acadrust::DxfReader::from_reader(Cursor::new(data))
            .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, format!("DXF read error: {}", e)))?
            .read()
            .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, format!("DXF parse error: {}", e)))?
    };

    eprintln!(
        "[convert] {} - Version: {}, Layers: {}, Entities: {}",
        filename,
        doc.version.as_str(),
        doc.layers.iter().count(),
        doc.entities().count()
    );

    // 3. 转换为 GeoJSON (使用 dxf2json.rs 中的共享逻辑)
    let layers = dxf2json_convert::convert_document(&doc, &[], false);

    // 4. 构建响应
    let layer_array: Vec<Value> = layers
        .into_iter()
        .map(|(name, fc)| {
            json!({
                "name": name,
                "geojson": fc
            })
        })
        .collect();

    Ok(Json(json!({
        "filename": filename,
        "version": doc.version.as_str(),
        "layerCount": layer_array.len(),
        "layers": layer_array
    })))
}
