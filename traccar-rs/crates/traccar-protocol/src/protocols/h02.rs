//! H02 Protocol
//!
//! Huabao compatible text-based protocol, commonly used by Chinese trackers.
//!
//! Text format: `*HQ,IMEI,V1,HHMMSS,A/V,lat,N/S,lon,E/W,speed,course,DDMMYY,status#`
//! Binary format also supported (0x24 start byte).
//!
//! Java reference: `H02ProtocolDecoder.java`

use async_trait::async_trait;
use bytes::{Buf, Bytes, BytesMut};
use traccar_helper::bit::bit_check;
use traccar_helper::date::{correct_year, make_datetime};
use traccar_model::position_keys;
use traccar_model::{Command, Position};

use crate::codec::DelimiterFrameDecoder;
use crate::session::DeviceSession;
use crate::{
    ProtocolDecoder, ProtocolDefinition, ProtocolEncoder, ProtocolError, ProtocolRegistry,
    ProtocolResult, Transport,
};

/// Protocol name constant.
pub const PROTOCOL_NAME: &str = "h02";

/// Default port for H02.
pub const DEFAULT_PORT: u16 = 5013;

// ─── Status processing ─────────────────────────────────────────────

fn process_status(position: &mut Position, status: i64) {
    if !bit_check(status, 0) {
        position.add_alarm(traccar_model::alarm::VIBRATION);
    } else if !bit_check(status, 1) || !bit_check(status, 18) {
        position.add_alarm(traccar_model::alarm::SOS);
    } else if !bit_check(status, 2) {
        position.add_alarm(traccar_model::alarm::OVERSPEED);
    } else if !bit_check(status, 19) {
        position.add_alarm(traccar_model::alarm::POWER_CUT);
    }

    position.set(position_keys::KEY_IGNITION, bit_check(status, 10));
    position.set(position_keys::KEY_STATUS, status);
}

fn decode_battery(value: i64) -> Option<i64> {
    if value == 0 {
        None
    } else if value <= 3 {
        Some((value - 1) * 10)
    } else if value <= 6 {
        Some((value - 1) * 20)
    } else if value <= 100 {
        Some(value)
    } else if (0xF1..=0xF6).contains(&value) {
        Some(value - 0xF0)
    } else {
        None
    }
}

// ─── Protocol decoder ───────────────────────────────────────────────

pub struct H02Decoder;

#[async_trait]
impl ProtocolDecoder for H02Decoder {
    async fn decode(
        &self,
        buf: &mut BytesMut,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        if buf.is_empty() {
            return Ok(None);
        }

        let first_byte = buf[0];

        if first_byte == b'$' {
            // Binary format
            return self.decode_binary(buf, session);
        } else if first_byte == b'*' {
            // Text format
            return self.decode_text(buf, session);
        }

        Err(ProtocolError::InvalidData(format!(
            "Invalid H02 start byte: 0x{:02X}",
            first_byte
        )))
    }
}

impl H02Decoder {
    /// Decode binary H02 message (starts with 0x24 '$').
    fn decode_binary(
        &self,
        buf: &BytesMut,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        if buf.len() < 32 {
            return Err(ProtocolError::InsufficientData);
        }

        let mut position = Position::new(PROTOCOL_NAME);
        let mut pos = 1; // skip '$' marker

        // IMEI: 5 bytes BCD (10 digits)
        let long_id = buf.len() == 42;
        let id = if long_id {
            let hex = hex::encode(&buf[pos..pos + 8]);
            pos += 8;
            hex[..15].to_string()
        } else {
            let hex = hex::encode(&buf[pos..pos + 5]);
            pos += 5;
            hex
        };

        session.unique_id = id.clone();
        session.device_id = simple_device_id(&session.unique_id);
        position.device_id = session.device_id;

        if session.device_id == 0 {
            return Err(ProtocolError::UnknownDevice(id));
        }

        // Time: BCD HHMMSS
        if pos + 3 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let hour = bcd_byte(buf[pos]) as u32;
        let minute = bcd_byte(buf[pos + 1]) as u32;
        let second = bcd_byte(buf[pos + 2]) as u32;
        pos += 3;

        // Date: BCD DDMMYY
        if pos + 3 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let day = bcd_byte(buf[pos]) as u32;
        let month = bcd_byte(buf[pos + 1]) as u32;
        let year = correct_year(bcd_byte(buf[pos + 2]) as i32);
        pos += 3;

        if let Some(dt) = make_datetime(year, month, day, hour, minute, second) {
            position.set_time(dt);
        }

        // Latitude: BCD DDmm.mmmm
        if pos + 4 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let lat_degrees = bcd_byte(buf[pos]) as f64;
        let lat_minutes = bcd_byte(buf[pos + 1]) as f64
            + bcd_byte(buf[pos + 2]) as f64 / 100.0
            + bcd_byte(buf[pos + 3]) as f64 / 10000.0;
        let latitude = lat_degrees + lat_minutes / 60.0;
        pos += 4;

        // Battery level
        if pos >= buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let batt_val = buf[pos] as i64;
        if let Some(level) = decode_battery(batt_val) {
            position.set(position_keys::KEY_BATTERY_LEVEL, level);
        }
        pos += 1;

        // Longitude: BCD DDDmm.mmmm
        if pos + 4 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let lon_degrees = bcd_byte(buf[pos]) as f64 * 10.0 + (buf[pos + 1] >> 4) as f64;
        let lon_minutes = (buf[pos + 1] & 0x0F) as f64 * 10.0
            + bcd_byte(buf[pos + 2]) as f64 / 10.0
            + bcd_byte(buf[pos + 3]) as f64 / 1000.0;
        let longitude = lon_degrees + lon_minutes / 60.0;
        pos += 4;

        // Flags byte
        if pos >= buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let flags = buf[pos] & 0x0F;
        pos += 1;

        position.valid = (flags & 0x02) != 0;
        position.latitude = if (flags & 0x04) != 0 {
            latitude
        } else {
            -latitude
        };
        position.longitude = if (flags & 0x08) != 0 {
            longitude
        } else {
            -longitude
        };

        // Speed: BCD (3 digits)
        if pos + 1 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let speed_high = bcd_byte(buf[pos]);
        pos += 1;
        if pos >= buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let speed_low = bcd_byte(buf[pos]);
        position.speed = (speed_high * 10 + speed_low / 10) as f64;
        pos += 1;

        // Course: high nibble + 2 BCD digits
        if pos + 2 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let course_high = (buf[pos] & 0x0F) as f64 * 100.0;
        let course_low = bcd_byte(buf[pos + 1]) as f64;
        position.course = course_high + course_low;
        pos += 2;

        // Status: 4 bytes
        if pos + 4 <= buf.len() {
            let status = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
            process_status(&mut position, status as i64);
        }

        Ok(Some(vec![position]))
    }

    /// Decode text H02 message (starts with '*').
    fn decode_text(
        &self,
        buf: &BytesMut,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        let data = String::from_utf8_lossy(buf).to_string();
        let data = data.trim_matches(|c| c == '*' || c == '#' || c == '\r' || c == '\n');

        let parts: Vec<&str> = data.split(',').collect();

        if parts.len() < 3 {
            return Err(ProtocolError::InvalidData("H02 text too short".to_string()));
        }

        // parts[0] = "HQ" (manufacturer), parts[1] = IMEI, parts[2] = message type
        let imei = parts[1];
        session.unique_id = imei.to_string();
        session.device_id = simple_device_id(&session.unique_id);

        let msg_type = parts[2];

        // Handle V1 (standard location) and V4 (response) messages
        if msg_type != "V1" && msg_type != "V4" && !msg_type.starts_with('V') {
            // Heartbeat, link, or other non-location message
            tracing::debug!(
                protocol = PROTOCOL_NAME,
                msg_type = msg_type,
                "H02 non-location message"
            );
            return Ok(None);
        }

        if parts.len() < 12 {
            return Ok(None);
        }

        let mut position = Position::new(PROTOCOL_NAME);
        position.device_id = session.device_id;

        if session.device_id == 0 {
            return Err(ProtocolError::UnknownDevice(imei.to_string()));
        }

        // Time: HHMMSS (parts[3])
        let time_str = parts[3];
        let date_str = parts.get(11).unwrap_or(&"010100");

        if time_str.len() >= 6 && date_str.len() >= 6 {
            let hour = time_str[0..2].parse::<u32>().unwrap_or(0);
            let min = time_str[2..4].parse::<u32>().unwrap_or(0);
            let sec = time_str[4..6].parse::<u32>().unwrap_or(0);
            let day = date_str[0..2].parse::<u32>().unwrap_or(1);
            let month = date_str[2..4].parse::<u32>().unwrap_or(1);
            let year = correct_year(date_str[4..6].parse::<i32>().unwrap_or(0));

            if let Some(dt) = make_datetime(year, month, day, hour, min, sec) {
                position.set_time(dt);
            }
        }

        // Validity: A = valid, V = invalid (parts[4])
        position.valid = parts[4] == "A";

        // Latitude: DDmm.mmmm (parts[5])
        if let Ok(raw_lat) = parts[5].parse::<f64>() {
            let degrees = (raw_lat / 100.0).floor();
            let minutes = raw_lat - degrees * 100.0;
            position.latitude = degrees + minutes / 60.0;
            if parts.get(6) == Some(&"S") {
                position.latitude = -position.latitude;
            }
        }

        // Longitude: DDDmm.mmmm (parts[7])
        if let Ok(raw_lon) = parts[7].parse::<f64>() {
            let degrees = (raw_lon / 100.0).floor();
            let minutes = raw_lon - degrees * 100.0;
            position.longitude = degrees + minutes / 60.0;
            if parts.get(8) == Some(&"W") {
                position.longitude = -position.longitude;
            }
        }

        // Speed (parts[9]) - in km/h, convert to knots
        if let Some(speed_str) = parts.get(9) {
            if let Ok(speed) = speed_str.parse::<f64>() {
                position.speed = traccar_helper::knots_from_kph(speed);
            }
        }

        // Course (parts[10])
        if let Some(course_str) = parts.get(10) {
            if let Ok(course) = course_str.parse::<f64>() {
                position.course = course;
            }
        }

        // Status word (parts[12]) if present
        if let Some(status_str) = parts.get(12) {
            if let Ok(status) = u32::from_str_radix(status_str, 16) {
                process_status(&mut position, status as i64);
            }
        }

        Ok(Some(vec![position]))
    }
}

/// Extract BCD value from a single byte: 0x45 -> 45.
fn bcd_byte(b: u8) -> u8 {
    ((b >> 4) & 0x0F) * 10 + (b & 0x0F)
}

// ─── Protocol encoder ───────────────────────────────────────────────

pub struct H02Encoder;

impl ProtocolEncoder for H02Encoder {
    fn encode(&self, command: &Command, unique_id: &str) -> ProtocolResult<Bytes> {
        let time_str = chrono::Utc::now().format("%H%M%S").to_string();

        let msg = match command.command_type.as_str() {
            Command::TYPE_ALARM_ARM => {
                format!("*HQ,{},SCF,{},0,0#", unique_id, time_str)
            }
            Command::TYPE_ALARM_DISARM => {
                format!("*HQ,{},SCF,{},1,1#", unique_id, time_str)
            }
            Command::TYPE_POSITION_PERIODIC => {
                let freq = command
                    .get_string(Command::KEY_FREQUENCY)
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(60);
                format!("*HQ,{},S71,{},{}#", unique_id, time_str, freq)
            }
            Command::TYPE_ENGINE_STOP => {
                format!("*HQ,{},S20,{},1,3,0,0,0#", unique_id, time_str)
            }
            Command::TYPE_ENGINE_RESUME => {
                format!("*HQ,{},S20,{},1,1,0,0,0#", unique_id, time_str)
            }
            Command::TYPE_POWER_OFF => {
                format!("*HQ,{},S06,{},0#", unique_id, time_str)
            }
            other => {
                return Err(ProtocolError::UnsupportedCommand(other.to_string()));
            }
        };

        Ok(Bytes::from(msg))
    }

    fn supported_commands(&self) -> &[&str] {
        &[
            Command::TYPE_ALARM_ARM,
            Command::TYPE_ALARM_DISARM,
            Command::TYPE_POSITION_PERIODIC,
            Command::TYPE_ENGINE_STOP,
            Command::TYPE_ENGINE_RESUME,
            Command::TYPE_POWER_OFF,
        ]
    }
}

// ─── H02 frame decoder ─────────────────────────────────────────────

/// Custom frame decoder for H02 protocol that handles both text and binary formats.
pub struct H02FrameDecoder {
    text_decoder: DelimiterFrameDecoder,
}

impl H02FrameDecoder {
    pub fn new() -> Self {
        Self {
            text_decoder: DelimiterFrameDecoder::from_strings(2048, false, &["#", "\r\n"]),
        }
    }
}

impl tokio_util::codec::Decoder for H02FrameDecoder {
    type Item = BytesMut;
    type Error = std::io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<BytesMut>, std::io::Error> {
        if buf.is_empty() {
            return Ok(None);
        }

        if buf[0] == b'$' {
            // Binary format: fixed length (32 or 42 bytes)
            let expected_len = if buf.len() >= 42 { 42 } else { 32 };
            if buf.len() >= expected_len {
                Ok(Some(buf.split_to(expected_len)))
            } else {
                Ok(None)
            }
        } else {
            // Text format: delimiter-based
            self.text_decoder.decode(buf)
        }
    }
}

impl crate::FrameDecoder for H02FrameDecoder {}

// ─── Registration ───────────────────────────────────────────────────

pub fn register(registry: &mut ProtocolRegistry) {
    let encoder = H02Encoder;
    let supported = encoder
        .supported_commands()
        .iter()
        .map(|s| s.to_string())
        .collect();

    registry.register(ProtocolDefinition {
        name: PROTOCOL_NAME.to_string(),
        transport: Transport::Both,
        default_port: DEFAULT_PORT,
        decoder_factory: Box::new(|| Box::new(H02Decoder)),
        encoder_factory: Box::new(|| Box::new(H02Encoder)),
        frame_decoder_factory: Box::new(|| Box::new(H02FrameDecoder::new())),
        supported_commands: supported,
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
    async fn test_decode_text_v1() {
        let decoder = H02Decoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let msg = "*HQ,123456789012345,V1,121500,A,2234.5678,N,11404.5678,E,60.5,270.3,150324,FFFFFFFF#";
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
        assert!(pos.latitude > 22.0 && pos.latitude < 23.0);
        assert!(pos.longitude > 114.0 && pos.longitude < 115.0);
    }

    #[tokio::test]
    async fn test_decode_text_invalid() {
        let decoder = H02Decoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let msg = "*HQ,123456789012345,V1,121500,V,2234.5678,N,11404.5678,E,0,0,150324,00000000#";
        let mut buf = BytesMut::from(msg);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        let positions = result.unwrap().unwrap();
        assert!(!positions[0].valid);
    }

    #[tokio::test]
    async fn test_decode_text_south_west() {
        let decoder = H02Decoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let msg = "*HQ,123456789012345,V1,121500,A,2234.5678,S,11404.5678,W,50,180,150324,00000000#";
        let mut buf = BytesMut::from(msg);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        let positions = result.unwrap().unwrap();
        assert!(positions[0].latitude < 0.0);
        assert!(positions[0].longitude < 0.0);
    }

    #[test]
    fn test_encode_engine_stop() {
        let encoder = H02Encoder;
        let command = Command {
            id: 0,
            device_id: 1,
            command_type: Command::TYPE_ENGINE_STOP.to_string(),
            description: None,
            attributes: serde_json::Value::Object(serde_json::Map::new()),
        };

        let result = encoder.encode(&command, "123456789012345").unwrap();
        let msg = String::from_utf8(result.to_vec()).unwrap();
        assert!(msg.starts_with("*HQ,123456789012345,S20,"));
        assert!(msg.ends_with("#"));
        assert!(msg.contains(",1,3,0,0,0#"));
    }

    #[test]
    fn test_bcd_byte() {
        assert_eq!(bcd_byte(0x45), 45);
        assert_eq!(bcd_byte(0x12), 12);
        assert_eq!(bcd_byte(0x00), 0);
        assert_eq!(bcd_byte(0x99), 99);
    }

    #[test]
    fn test_decode_battery() {
        assert_eq!(decode_battery(0), None);
        assert_eq!(decode_battery(1), Some(0));
        assert_eq!(decode_battery(3), Some(20));
        assert_eq!(decode_battery(6), Some(100));
        assert_eq!(decode_battery(50), Some(50));
        assert_eq!(decode_battery(0xF1), Some(1));
    }

    #[test]
    fn test_process_status() {
        let mut pos = Position::new(PROTOCOL_NAME);
        let status: i64 = 0xFFFFFFFF;
        process_status(&mut pos, status);
        // All bits set means ignition should be on
        assert_eq!(pos.get_boolean(position_keys::KEY_IGNITION), Some(true));
    }
}
