//! OsmAnd Protocol
//!
//! HTTP-based protocol used by OsmAnd mobile app and other clients.
//! Devices send HTTP GET/POST requests with URL-encoded parameters.
//!
//! Parameters: id, timestamp, lat, lon, speed, bearing, altitude, accuracy, hdop, batt, etc.
//!
//! Example request:
//!   GET /?id=123456&timestamp=1234567890&lat=12.345&lon=67.890&speed=10&bearing=90&altitude=100
//!
//! Also supports JSON POST format from OsmAnd background location plugin.
//!
//! Java reference: `OsmAndProtocolDecoder.java`

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use traccar_helper::date::{from_unix_millis, from_unix_secs};
use traccar_model::position_keys;
use traccar_model::{Command, Position};

use crate::codec::LineBasedFrameDecoder;
use crate::session::DeviceSession;
use crate::{
    ProtocolDecoder, ProtocolDefinition, ProtocolEncoder, ProtocolError, ProtocolRegistry,
    ProtocolResult, Transport,
};

/// Protocol name constant.
pub const PROTOCOL_NAME: &str = "osmand";

/// Default port for OsmAnd.
pub const DEFAULT_PORT: u16 = 5055;

// ─── Protocol decoder ───────────────────────────────────────────────

pub struct OsmAndDecoder;

#[async_trait]
impl ProtocolDecoder for OsmAndDecoder {
    async fn decode(
        &self,
        buf: &mut BytesMut,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        let data = String::from_utf8_lossy(buf).to_string();

        if data.is_empty() {
            return Ok(None);
        }

        // Check for JSON POST body
        if data.trim_start().starts_with('{') {
            return self.decode_json(&data, session);
        }

        // Parse as HTTP request or raw URL parameters
        let query_string = extract_query_string(&data);
        if query_string.is_empty() {
            return Ok(None);
        }

        self.decode_params(&query_string, session)
    }
}

impl OsmAndDecoder {
    /// Decode URL query parameters into a position.
    fn decode_params(
        &self,
        query_string: &str,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        let params: HashMap<String, String> = parse_query_string(query_string);

        // Device ID is required
        let device_id = params
            .get("id")
            .or_else(|| params.get("deviceid"))
            .cloned()
            .unwrap_or_default();

        if device_id.is_empty() {
            return Ok(None);
        }

        session.unique_id = device_id;
        session.device_id = simple_device_id(&session.unique_id);

        let mut position = Position::new(PROTOCOL_NAME);
        position.device_id = session.device_id;
        position.valid = true;

        let mut latitude: Option<f64> = None;
        let mut longitude: Option<f64> = None;

        for (key, value) in &params {
            match key.as_str() {
                "id" | "deviceid" => {
                    // Already handled above
                }
                "valid" => {
                    position.valid =
                        value.parse::<bool>().unwrap_or(false) || value == "1";
                }
                "timestamp" => {
                    position.set_time(parse_timestamp(value));
                }
                "lat" => {
                    latitude = value.parse().ok();
                }
                "lon" => {
                    longitude = value.parse().ok();
                }
                "location" => {
                    // Format: lat,lon
                    let parts: Vec<&str> = value.split(',').collect();
                    if parts.len() >= 2 {
                        latitude = parts[0].parse().ok();
                        longitude = parts[1].parse().ok();
                    }
                }
                "speed" => {
                    // OsmAnd sends speed in knots by default
                    if let Ok(speed) = value.parse::<f64>() {
                        position.speed = speed;
                    }
                }
                "bearing" | "heading" => {
                    if let Ok(course) = value.parse::<f64>() {
                        position.course = course;
                    }
                }
                "altitude" => {
                    if let Ok(alt) = value.parse::<f64>() {
                        position.altitude = alt;
                    }
                }
                "accuracy" => {
                    if let Ok(acc) = value.parse::<f64>() {
                        position.accuracy = acc;
                    }
                }
                "hdop" => {
                    if let Ok(hdop) = value.parse::<f64>() {
                        position.set(position_keys::KEY_HDOP, hdop);
                    }
                }
                "batt" => {
                    if let Ok(batt) = value.parse::<f64>() {
                        position.set(position_keys::KEY_BATTERY_LEVEL, batt);
                    }
                }
                "driverUniqueId" => {
                    position.set(position_keys::KEY_DRIVER_UNIQUE_ID, value.clone());
                }
                "charge" => {
                    if let Ok(charging) = value.parse::<bool>() {
                        position.set(position_keys::KEY_CHARGE, charging);
                    }
                }
                _ => {
                    // Store unknown params as custom attributes
                    if let Ok(num) = value.parse::<f64>() {
                        position.set(key.as_str(), num);
                    } else {
                        match value.as_str() {
                            "true" => position.set(key.as_str(), true),
                            "false" => position.set(key.as_str(), false),
                            _ => position.set(key.as_str(), value.clone()),
                        }
                    }
                }
            }
        }

        // Set coordinates
        if let (Some(lat), Some(lon)) = (latitude, longitude) {
            position.latitude = lat;
            position.longitude = lon;
        }

        // Default time to now if not provided
        if position.fix_time == position.server_time {
            position.set_time(Utc::now());
        }

        Ok(Some(vec![position]))
    }

    /// Decode JSON POST body (from OsmAnd background location plugin).
    fn decode_json(
        &self,
        data: &str,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        let root: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| ProtocolError::Decode(format!("JSON parse error: {}", e)))?;

        let device_id = root
            .get("device_id")
            .or_else(|| root.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        if device_id.is_empty() {
            return Ok(None);
        }

        session.unique_id = device_id.to_string();
        session.device_id = simple_device_id(&session.unique_id);

        let mut position = Position::new(PROTOCOL_NAME);
        position.device_id = session.device_id;

        // Parse location object if present
        if let Some(location) = root.get("location") {
            // Timestamp
            if let Some(ts) = location.get("timestamp").and_then(|v| v.as_str()) {
                if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
                    position.set_time(dt.with_timezone(&Utc));
                }
            }

            // Coordinates from coords sub-object
            if let Some(coords) = location.get("coords") {
                position.valid = true;

                if let Some(lat) = coords.get("latitude").and_then(|v| v.as_f64()) {
                    position.latitude = lat;
                }
                if let Some(lon) = coords.get("longitude").and_then(|v| v.as_f64()) {
                    position.longitude = lon;
                }
                if let Some(speed) = coords.get("speed").and_then(|v| v.as_f64()) {
                    if speed >= 0.0 {
                        position.speed = traccar_helper::knots_from_mps(speed);
                    }
                }
                if let Some(heading) = coords.get("heading").and_then(|v| v.as_f64()) {
                    if heading >= 0.0 {
                        position.course = heading;
                    }
                }
                if let Some(accuracy) = coords.get("accuracy").and_then(|v| v.as_f64()) {
                    position.accuracy = accuracy;
                }
                if let Some(alt) = coords.get("altitude").and_then(|v| v.as_f64()) {
                    position.altitude = alt;
                }
            }

            // Additional fields
            if let Some(event) = location.get("event").and_then(|v| v.as_str()) {
                position.set(position_keys::KEY_EVENT, event.to_string());
            }
            if let Some(moving) = location.get("is_moving").and_then(|v| v.as_bool()) {
                position.set(position_keys::KEY_MOTION, moving);
            }
            if let Some(odometer) = location.get("odometer").and_then(|v| v.as_i64()) {
                position.set(position_keys::KEY_ODOMETER, odometer);
            }

            // Battery info
            if let Some(battery) = location.get("battery") {
                if let Some(level) = battery.get("level").and_then(|v| v.as_f64()) {
                    if level >= 0.0 {
                        position.set(position_keys::KEY_BATTERY_LEVEL, (level * 100.0) as i64);
                    }
                }
                if let Some(charging) = battery.get("is_charging").and_then(|v| v.as_bool()) {
                    if charging {
                        position.set(position_keys::KEY_CHARGE, true);
                    }
                }
            }
        } else {
            // Flat JSON format
            position.valid = true;
            if let Some(lat) = root.get("lat").and_then(|v| v.as_f64()) {
                position.latitude = lat;
            }
            if let Some(lon) = root.get("lon").and_then(|v| v.as_f64()) {
                position.longitude = lon;
            }
            if let Some(ts) = root.get("timestamp").and_then(|v| v.as_i64()) {
                position.set_time(from_unix_secs(ts));
            }
        }

        Ok(Some(vec![position]))
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

/// Extract query string from an HTTP request line or raw parameter string.
fn extract_query_string(data: &str) -> String {
    if data.starts_with("GET ") || data.starts_with("POST ") {
        // HTTP request: extract query part from URI
        data.split_whitespace()
            .nth(1)
            .and_then(|path| path.split('?').nth(1))
            .unwrap_or("")
            .to_string()
    } else if data.contains('=') {
        // Raw parameters
        data.trim().to_string()
    } else {
        String::new()
    }
}

/// Parse URL-encoded query string into key-value map.
fn parse_query_string(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some(k), Some(v)) if !k.is_empty() => {
                    Some((k.to_string(), url_decode(v)))
                }
                _ => None,
            }
        })
        .collect()
}

/// Simple URL decode (handles %XX encoding).
fn url_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();

    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push_str(&hex);
        } else if c == '+' {
            result.push(' ');
        } else {
            result.push(c);
        }
    }

    result
}

/// Parse timestamp from various formats (unix seconds, unix millis, ISO 8601).
fn parse_timestamp(value: &str) -> DateTime<Utc> {
    // Try as integer (unix timestamp)
    if let Ok(ts) = value.parse::<i64>() {
        if ts < i32::MAX as i64 {
            return from_unix_secs(ts);
        } else {
            return from_unix_millis(ts);
        }
    }

    // Try as ISO 8601 / RFC 3339
    if value.contains('T') {
        if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
            return dt.with_timezone(&Utc);
        }
    }

    // Try as "yyyy-MM-dd HH:mm:ss"
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return chrono::TimeZone::from_utc_datetime(&Utc, &naive);
    }

    Utc::now()
}

// ─── Protocol encoder ───────────────────────────────────────────────

pub struct OsmAndEncoder;

impl ProtocolEncoder for OsmAndEncoder {
    fn encode(&self, _command: &Command, _unique_id: &str) -> ProtocolResult<Bytes> {
        Err(ProtocolError::UnsupportedCommand(
            "OsmAnd is a read-only HTTP protocol".to_string(),
        ))
    }
}

// ─── Registration ───────────────────────────────────────────────────

pub fn register(registry: &mut ProtocolRegistry) {
    registry.register(ProtocolDefinition {
        name: PROTOCOL_NAME.to_string(),
        transport: Transport::Tcp,
        default_port: DEFAULT_PORT,
        decoder_factory: Box::new(|| Box::new(OsmAndDecoder)),
        encoder_factory: Box::new(|| Box::new(OsmAndEncoder)),
        frame_decoder_factory: Box::new(|| Box::new(LineBasedFrameDecoder::new(4096))),
        supported_commands: vec![],
    });
}

/// Simple hash-based device ID for development/testing.
fn simple_device_id(unique_id: &str) -> i64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    unique_id.hash(&mut hasher);
    (hasher.finish() & 0x7FFFFFFFFFFFFFFF) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_decode_get_request() {
        let decoder = OsmAndDecoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let msg = "GET /?id=123456&timestamp=1710500000&lat=22.54&lon=114.06&speed=10&bearing=180&altitude=50 HTTP/1.1\r\n";
        let mut buf = BytesMut::from(msg);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        let positions = result.unwrap();
        assert!(positions.is_some());
        let positions = positions.unwrap();
        assert_eq!(positions.len(), 1);

        let pos = &positions[0];
        assert_eq!(pos.protocol, PROTOCOL_NAME);
        assert!(pos.valid);
        assert!((pos.latitude - 22.54).abs() < 0.001);
        assert!((pos.longitude - 114.06).abs() < 0.001);
        assert!((pos.altitude - 50.0).abs() < 0.001);
        assert!((pos.course - 180.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_decode_raw_params() {
        let decoder = OsmAndDecoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let msg = "id=test_device&lat=-33.8688&lon=151.2093&speed=5&altitude=100&accuracy=10";
        let mut buf = BytesMut::from(msg);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        let positions = result.unwrap().unwrap();
        let pos = &positions[0];
        assert_eq!(session.unique_id, "test_device");
        assert!((pos.latitude - (-33.8688)).abs() < 0.001);
        assert!((pos.longitude - 151.2093).abs() < 0.001);
        assert!((pos.accuracy - 10.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_decode_with_battery() {
        let decoder = OsmAndDecoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let msg = "id=dev1&lat=0&lon=0&batt=85&hdop=1.2";
        let mut buf = BytesMut::from(msg);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        let positions = result.unwrap().unwrap();
        let pos = &positions[0];
        assert_eq!(pos.get_double(position_keys::KEY_BATTERY_LEVEL), Some(85.0));
        assert_eq!(pos.get_double(position_keys::KEY_HDOP), Some(1.2));
    }

    #[tokio::test]
    async fn test_decode_json_post() {
        let decoder = OsmAndDecoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let json = r#"{"device_id":"myphone","location":{"timestamp":"2024-03-15T10:30:00Z","coords":{"latitude":51.5074,"longitude":-0.1278,"speed":2.5,"heading":90,"accuracy":5,"altitude":15}}}"#;
        let mut buf = BytesMut::from(json);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        let positions = result.unwrap().unwrap();
        let pos = &positions[0];
        assert_eq!(session.unique_id, "myphone");
        assert!(pos.valid);
        assert!((pos.latitude - 51.5074).abs() < 0.001);
        assert!((pos.longitude - (-0.1278)).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_decode_missing_id() {
        let decoder = OsmAndDecoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let msg = "lat=22.54&lon=114.06";
        let mut buf = BytesMut::from(msg);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // no ID means no position
    }

    #[test]
    fn test_parse_query_string() {
        let params = parse_query_string("id=123&lat=22.5&lon=114.0");
        assert_eq!(params.get("id").unwrap(), "123");
        assert_eq!(params.get("lat").unwrap(), "22.5");
    }

    #[test]
    fn test_url_decode() {
        assert_eq!(url_decode("hello%20world"), "hello world");
        assert_eq!(url_decode("test+value"), "test value");
        assert_eq!(url_decode("plain"), "plain");
    }

    #[test]
    fn test_parse_timestamp_unix() {
        let dt = parse_timestamp("1710500000");
        assert_eq!(dt.timestamp(), 1710500000);
    }

    #[test]
    fn test_parse_timestamp_iso() {
        let dt = parse_timestamp("2024-03-15T10:30:00Z");
        assert_eq!(dt.timestamp(), 1710498600);
    }

    #[test]
    fn test_encode_unsupported() {
        let encoder = OsmAndEncoder;
        let command = Command {
            id: 0,
            device_id: 1,
            command_type: "custom".to_string(),
            description: None,
            attributes: serde_json::Value::Object(serde_json::Map::new()),
        };
        assert!(encoder.encode(&command, "test").is_err());
    }
}
