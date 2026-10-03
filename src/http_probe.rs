//! Pure HTTP-1 routes and fixed fixtures, independent of WASI host calls.
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use serde::Serialize;
use serde_json::json;
use std::io::Cursor;
use std::sync::OnceLock;

pub const PROTOCOL_VERSION: u32 = 1;
pub const SERVICE_NAME: &str = "bandcomic-local-http";
pub const SAMPLE_WIDTH: u32 = 32;
pub const SAMPLE_HEIGHT: u32 = 32;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceIdentity {
    pub host_id: Option<String>,
    pub instance_id: String,
    pub server_id: u32,
    pub port: u16,
}

pub struct ProbeResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct Samples {
    pub jpeg: Vec<u8>,
    pub png: Vec<u8>,
    pub lvgl: Vec<u8>,
}

static SAMPLES: OnceLock<Result<Samples, String>> = OnceLock::new();

pub fn samples() -> Result<&'static Samples, &'static str> {
    SAMPLES.get_or_init(make_samples).as_ref().map_err(String::as_str)
}

fn make_samples() -> Result<Samples, String> {
    // Four quadrants: red, green, blue, white. One small image in all formats.
    let rgb = RgbImage::from_fn(SAMPLE_WIDTH, SAMPLE_HEIGHT, |x, y| {
        Rgb(match (x < SAMPLE_WIDTH / 2, y < SAMPLE_HEIGHT / 2) {
            (true, true) => [255, 0, 0],
            (false, true) => [0, 255, 0],
            (true, false) => [0, 0, 255],
            (false, false) => [255, 255, 255],
        })
    });
    let image = DynamicImage::ImageRgb8(rgb);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
        .encode_image(&image).map_err(|e| e.to_string())?;
    let mut png = Cursor::new(Vec::new());
    image.write_to(&mut png, ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(Samples { jpeg, png: png.into_inner(), lvgl: crate::lvgl::convert_to_lvgl_i8(&image) })
}

fn response(status: u16, content_type: &str, body: Vec<u8>) -> ProbeResponse {
    ProbeResponse {
        status,
        headers: vec![
            ("Content-Type".into(), content_type.into()),
            ("Content-Length".into(), body.len().to_string()),
            ("Cache-Control".into(), "no-store".into()),
        ],
        body,
    }
}

pub fn error(status: u16, message: &str) -> ProbeResponse {
    response(status, "application/json; charset=utf-8",
        json!({ "code": status, "message": message }).to_string().into_bytes())
}

pub fn route(method: &str, path: &str, identity: &ServiceIdentity) -> ProbeResponse {
    if !matches!(path, "/control/health" | "/control/probe.jpg" | "/control/probe.png" | "/control/probe.bin") {
        return error(404, "Route not found");
    }
    if method != "GET" {
        let mut result = error(405, "Method not allowed");
        result.headers.push(("Allow".into(), "GET".into()));
        return result;
    }
    let samples = match samples() {
        Ok(samples) => samples,
        Err(_) => return error(500, "Probe samples unavailable"),
    };
    match path {
        "/control/health" => response(200, "application/json; charset=utf-8", json!({
            "service": SERVICE_NAME,
            "protocolVersion": PROTOCOL_VERSION,
            "apiLevel": 4,
            "pluginVersion": env!("CARGO_PKG_VERSION"),
            "hostId": identity.host_id,
            "instanceId": identity.instance_id,
            "serverId": identity.server_id,
            "port": identity.port,
            "capabilities": { "httpProbe": true, "httpImport": false, "httpDataSync": 1 },
            "samples": [
                { "path": "/control/probe.jpg", "contentType": "image/jpeg", "length": samples.jpeg.len() },
                { "path": "/control/probe.png", "contentType": "image/png", "length": samples.png.len() },
                { "path": "/control/probe.bin", "contentType": "application/octet-stream", "length": samples.lvgl.len() },
            ],
            "sampleWidth": SAMPLE_WIDTH,
            "sampleHeight": SAMPLE_HEIGHT,
        }).to_string().into_bytes()),
        "/control/probe.jpg" => response(200, "image/jpeg", samples.jpeg.clone()),
        "/control/probe.png" => response(200, "image/png", samples.png.clone()),
        "/control/probe.bin" => response(200, "application/octet-stream", samples.lvgl.clone()),
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ServiceIdentity {
        ServiceIdentity { host_id: Some("test-host".into()), instance_id: "test-instance".into(), server_id: 1, port: 53124 }
    }

    #[test]
    fn fixtures_decode_and_lvgl_palette_reconstructs_the_png() {
        let samples = samples().unwrap();
        let jpeg = image::load_from_memory_with_format(&samples.jpeg, ImageFormat::Jpeg).unwrap();
        let png = image::load_from_memory_with_format(&samples.png, ImageFormat::Png).unwrap().to_rgb8();
        assert_eq!((jpeg.width(), jpeg.height()), (32, 32));
        assert_eq!((png.width(), png.height()), (32, 32));
        assert_eq!(samples.lvgl.len(), 4 + 256 * 4 + 32 * 32);
        let header = u32::from_le_bytes(samples.lvgl[..4].try_into().unwrap());
        assert_eq!(header & 0x3ff, 10);
        assert_eq!((header >> 10) & 0x7ff, 32);
        assert_eq!(header >> 21, 32);
        for (index, pixel) in png.pixels().enumerate() {
            let palette_index = samples.lvgl[4 + 256 * 4 + index] as usize;
            let offset = 4 + palette_index * 4;
            let color = &samples.lvgl[offset..offset + 4];
            assert_eq!([color[2], color[1], color[0]], pixel.0);
            assert_eq!(color[3], 255);
        }
    }

    #[test]
    fn health_and_binary_lengths_describe_the_actual_response() {
        let health = route("GET", "/control/health", &identity());
        let json: serde_json::Value = serde_json::from_slice(&health.body).unwrap();
        assert_eq!(json["hostId"], "test-host");
        assert_eq!(json["instanceId"], "test-instance");
        assert_eq!(json["capabilities"]["httpImport"], false);
        assert!(json.get("token").is_none());
        for sample in json["samples"].as_array().unwrap() {
            let result = route("GET", sample["path"].as_str().unwrap(), &identity());
            assert_eq!(result.status, 200);
            assert_eq!(sample["length"].as_u64().unwrap(), result.body.len() as u64);
            assert!(result.headers.contains(&("Content-Type".into(), sample["contentType"].as_str().unwrap().into())));
            assert!(result.headers.contains(&("Content-Length".into(), result.body.len().to_string())));
        }
    }

    #[test]
    fn unknown_routes_and_methods_return_json_errors() {
        for (method, path, status) in [("GET", "/config", 404), ("POST", "/control/health", 405), ("HEAD", "/control/probe.jpg", 405)] {
            let result = route(method, path, &identity());
            assert_eq!(result.status, status);
            let body: serde_json::Value = serde_json::from_slice(&result.body).unwrap();
            assert_eq!(body["code"], status);
            assert!(result.headers.contains(&("Content-Length".into(), result.body.len().to_string())));
            if status == 405 { assert!(result.headers.contains(&("Allow".into(), "GET".into()))); }
        }
    }
}
