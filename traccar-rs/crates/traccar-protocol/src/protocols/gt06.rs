//! GT06 Protocol
//!
//! Common Chinese tracker protocol using a binary format.
//!
//! Frame format:
//!   Start marker: 0x78 0x78 (short) or 0x79 0x79 (extended)
//!   Length:        1 byte (short) or 2 bytes (extended)
//!   Protocol#:     1 byte
//!   Data:          variable
//!   Serial:        2 bytes
//!   CRC:           2 bytes
//!   End marker:    0x0D 0x0A
//!
//! Java reference: `Gt06ProtocolDecoder.java`, `Gt06FrameDecoder.java`

use async_trait::async_trait;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use traccar_helper::checksum::crc16_x25;
use traccar_helper::date::{correct_year, make_datetime};
use traccar_helper::parser::bcd_to_string;
use traccar_model::position_keys;
use traccar_model::{Command, Position};

use crate::session::DeviceSession;
use crate::{
    ProtocolDecoder, ProtocolDefinition, ProtocolEncoder, ProtocolError, ProtocolRegistry,
    ProtocolResult, Transport,
};

/// Protocol name constant.
pub const PROTOCOL_NAME: &str = "gt06";

/// Default port for GT06.
pub const DEFAULT_PORT: u16 = 5023;

// ─── Message type constants ─────────────────────────────────────────

pub const MSG_LOGIN: u8 = 0x01;
pub const MSG_GPS: u8 = 0x10;
pub const MSG_GPS_LBS_6: u8 = 0x11;
pub const MSG_GPS_LBS_1: u8 = 0x12;
pub const MSG_GPS_LBS_2: u8 = 0x22;
pub const MSG_GPS_LBS_3: u8 = 0x37;
pub const MSG_GPS_LBS_4: u8 = 0x2D;
pub const MSG_STATUS: u8 = 0x13;
pub const MSG_SATELLITE: u8 = 0x14;
pub const MSG_STRING: u8 = 0x15;
pub const MSG_GPS_LBS_STATUS_1: u8 = 0x16;
pub const MSG_GPS_LBS_STATUS_2: u8 = 0x26;
pub const MSG_GPS_LBS_STATUS_3: u8 = 0x27;
pub const MSG_LBS_EXTEND: u8 = 0x18;
pub const MSG_LBS_STATUS: u8 = 0x19;
pub const MSG_GPS_PHONE: u8 = 0x1A;
pub const MSG_HEARTBEAT: u8 = 0x23;
pub const MSG_GPS_LBS_STATUS_4: u8 = 0x32;
pub const MSG_GPS_LBS_5: u8 = 0x31;
pub const MSG_GPS_LBS_7: u8 = 0xA0;
pub const MSG_COMMAND_0: u8 = 0x80;
pub const MSG_COMMAND_1: u8 = 0x81;
pub const MSG_COMMAND_2: u8 = 0x82;
pub const MSG_TIME_REQUEST: u8 = 0x8A;
pub const MSG_INFO: u8 = 0x94;
pub const MSG_ALARM: u8 = 0x95;

// ─── GT06 frame decoder ────────────────────────────────────────────

/// Custom frame decoder for GT06 binary protocol.
/// Handles both 0x78 0x78 (1-byte length) and 0x79 0x79 (2-byte length) frames.
pub struct Gt06FrameDecoder;

impl tokio_util::codec::Decoder for Gt06FrameDecoder {
    type Item = BytesMut;
    type Error = std::io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<BytesMut>, std::io::Error> {
        if buf.len() < 5 {
            return Ok(None);
        }

        let first = buf[0];

        // Calculate expected frame length
        let frame_length = if first == 0x78 {
            // Short header: 2 (start) + 1 (length) + length_value + 2 (end)
            let data_len = buf[2] as usize;
            2 + 1 + data_len + 2
        } else if first == 0x79 {
            // Extended header: 2 (start) + 2 (length) + length_value + 2 (end)
            if buf.len() < 6 {
                return Ok(None);
            }
            let data_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            2 + 2 + data_len + 2
        } else {
            // Unknown start byte, skip one byte and try again
            buf.advance(1);
            return Ok(None);
        };

        if buf.len() < frame_length {
            return Ok(None);
        }

        // Verify end marker (0x0D 0x0A)
        if buf[frame_length - 2] == 0x0D && buf[frame_length - 1] == 0x0A {
            return Ok(Some(buf.split_to(frame_length)));
        }

        // Fallback: scan for end marker
        let mut end_idx = 0;
        for i in 2..buf.len().saturating_sub(1) {
            if buf[i] == 0x0D && buf[i + 1] == 0x0A {
                end_idx = i + 2;
                break;
            }
        }
        if end_idx > 0 {
            Ok(Some(buf.split_to(end_idx)))
        } else {
            Ok(None)
        }
    }
}

impl crate::FrameDecoder for Gt06FrameDecoder {}

// ─── Helpers ────────────────────────────────────────────────────────

fn is_gps_message(msg_type: u8) -> bool {
    matches!(
        msg_type,
        MSG_GPS
            | MSG_GPS_LBS_6
            | MSG_GPS_LBS_1
            | MSG_GPS_LBS_2
            | MSG_GPS_LBS_3
            | MSG_GPS_LBS_4
            | MSG_GPS_LBS_5
            | MSG_GPS_LBS_7
            | MSG_GPS_LBS_STATUS_1
            | MSG_GPS_LBS_STATUS_2
            | MSG_GPS_LBS_STATUS_3
            | MSG_GPS_LBS_STATUS_4
            | MSG_GPS_PHONE
    )
}

fn has_lbs(msg_type: u8) -> bool {
    matches!(
        msg_type,
        MSG_GPS_LBS_6
            | MSG_GPS_LBS_1
            | MSG_GPS_LBS_2
            | MSG_GPS_LBS_3
            | MSG_GPS_LBS_4
            | MSG_GPS_LBS_5
            | MSG_GPS_LBS_7
            | MSG_GPS_LBS_STATUS_1
            | MSG_GPS_LBS_STATUS_2
            | MSG_GPS_LBS_STATUS_3
            | MSG_GPS_LBS_STATUS_4
    )
}

fn has_status(msg_type: u8) -> bool {
    matches!(
        msg_type,
        MSG_GPS_LBS_STATUS_1
            | MSG_GPS_LBS_STATUS_2
            | MSG_GPS_LBS_STATUS_3
            | MSG_GPS_LBS_STATUS_4
    )
}

fn decode_alarm_byte(alarm: u8) -> Option<&'static str> {
    match alarm {
        0x01 => Some(traccar_model::alarm::SOS),
        0x02 => Some(traccar_model::alarm::POWER_CUT),
        0x03 => Some(traccar_model::alarm::VIBRATION),
        0x04 => Some(traccar_model::alarm::GEOFENCE_ENTER),
        0x05 => Some(traccar_model::alarm::GEOFENCE_EXIT),
        0x06 => Some(traccar_model::alarm::OVERSPEED),
        0x09 => Some(traccar_model::alarm::MOVEMENT),
        0x0E => Some(traccar_model::alarm::LOW_BATTERY),
        0x0F => Some(traccar_model::alarm::POWER_CUT),
        0x11 => Some(traccar_model::alarm::POWER_OFF),
        0x13 => Some(traccar_model::alarm::TAMPERING),
        0x14 => Some(traccar_model::alarm::DOOR),
        0x23 => Some(traccar_model::alarm::REMOVING),
        _ => None,
    }
}

// ─── Protocol decoder ───────────────────────────────────────────────

pub struct Gt06Decoder;

#[async_trait]
impl ProtocolDecoder for Gt06Decoder {
    async fn decode(
        &self,
        buf: &mut BytesMut,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        if buf.len() < 5 {
            return Ok(None);
        }

        let first_byte = buf[0];
        let header_size = if first_byte == 0x78 {
            3 // 2 start + 1 length
        } else if first_byte == 0x79 {
            if buf.len() < 6 {
                return Ok(None);
            }
            4 // 2 start + 2 length
        } else {
            return Err(ProtocolError::InvalidData(format!(
                "Invalid GT06 start byte: 0x{:02X}",
                first_byte
            )));
        };

        // Protocol number is right after the header
        if buf.len() <= header_size {
            return Ok(None);
        }
        let protocol_number = buf[header_size];

        match protocol_number {
            MSG_LOGIN => self.decode_login(buf, header_size, session),
            msg if is_gps_message(msg) => {
                self.decode_location(buf, header_size, msg, session)
            }
            MSG_STATUS | MSG_HEARTBEAT => {
                tracing::debug!(
                    protocol = PROTOCOL_NAME,
                    msg_type = protocol_number,
                    "GT06 status/heartbeat"
                );
                Ok(None)
            }
            MSG_ALARM => {
                // Alarm messages may contain location data
                self.decode_location(buf, header_size, protocol_number, session)
            }
            _ => {
                tracing::debug!(
                    protocol = PROTOCOL_NAME,
                    msg_type = format!("0x{:02X}", protocol_number),
                    "Unhandled GT06 message type"
                );
                Ok(None)
            }
        }
    }
}

impl Gt06Decoder {
    /// Decode a login message: extract IMEI and set up device session.
    fn decode_login(
        &self,
        buf: &BytesMut,
        header_size: usize,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        let data_start = header_size + 1; // after protocol number

        if buf.len() < data_start + 8 {
            return Err(ProtocolError::InsufficientData);
        }

        // IMEI is 8 bytes BCD encoded (16 digits, but only first 15 are valid)
        let imei_bytes = &buf[data_start..data_start + 8];
        let imei = bcd_to_string(imei_bytes);
        // Trim leading zeros, take up to 15 digits
        let imei = imei.trim_start_matches('0');
        let imei = if imei.len() > 15 {
            &imei[..15]
        } else {
            imei
        };

        session.unique_id = imei.to_string();
        session.device_id = simple_device_id(&session.unique_id);

        tracing::info!(
            protocol = PROTOCOL_NAME,
            imei = %session.unique_id,
            "GT06 login"
        );

        Ok(None)
    }

    /// Decode a GPS/location message.
    fn decode_location(
        &self,
        buf: &BytesMut,
        header_size: usize,
        msg_type: u8,
        session: &mut DeviceSession,
    ) -> ProtocolResult<Option<Vec<Position>>> {
        if session.device_id == 0 {
            return Err(ProtocolError::UnknownDevice(
                "Device not logged in".to_string(),
            ));
        }

        let data_start = header_size + 1;
        let mut pos = data_start;

        if buf.len() < pos + 12 {
            return Err(ProtocolError::InsufficientData);
        }

        let mut position = Position::new(PROTOCOL_NAME);
        position.device_id = session.device_id;

        // Date/time: 6 bytes (YY MM DD HH MM SS)
        let year = correct_year(buf[pos] as i32);
        let month = buf[pos + 1] as u32;
        let day = buf[pos + 2] as u32;
        let hour = buf[pos + 3] as u32;
        let minute = buf[pos + 4] as u32;
        let second = buf[pos + 5] as u32;
        pos += 6;

        if let Some(dt) = make_datetime(year, month, day, hour, minute, second) {
            position.set_time(dt);
        }

        // GPS data: info byte (4-bit length + 4-bit satellites)
        if pos >= buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let gps_info = buf[pos];
        let _gps_length = ((gps_info >> 4) & 0x0F) as usize;
        let satellites = (gps_info & 0x0F) as i64;
        pos += 1;

        position.set(position_keys::KEY_SATELLITES, satellites);

        if pos + 8 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }

        // Latitude: 4 bytes (unsigned, in units of 1/30000 of a minute)
        let lat_raw = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        pos += 4;

        // Longitude: 4 bytes
        let lon_raw = u32::from_be_bytes([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        pos += 4;

        let mut latitude = (lat_raw as f64) / 30000.0 / 60.0;
        let mut longitude = (lon_raw as f64) / 30000.0 / 60.0;

        // Speed: 1 byte (km/h)
        if pos >= buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let speed_kph = buf[pos] as f64;
        position.speed = traccar_helper::knots_from_kph(speed_kph);
        pos += 1;

        // Course and flags: 2 bytes
        if pos + 2 > buf.len() {
            return Err(ProtocolError::InsufficientData);
        }
        let course_status = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        pos += 2;

        position.course = (course_status & 0x03FF) as f64;

        let is_real_time = (course_status & 0x2000) != 0;
        let is_positioned = (course_status & 0x1000) != 0;
        let south = (course_status & 0x0400) != 0;
        let west = (course_status & 0x0800) != 0;

        position.valid = is_real_time && is_positioned;

        if south {
            latitude = -latitude;
        }
        if west {
            longitude = -longitude;
        }

        position.latitude = latitude;
        position.longitude = longitude;

        // Parse LBS data if present
        if has_lbs(msg_type) && pos + 8 <= buf.len() {
            let mcc = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
            let mnc = buf[pos + 2];
            let lac = u16::from_be_bytes([buf[pos + 3], buf[pos + 4]]);
            let cell_id = ((buf[pos + 5] as u32) << 16)
                | ((buf[pos + 6] as u32) << 8)
                | (buf[pos + 7] as u32);
            pos += 8;

            // Store LBS data as attributes for network-based positioning
            position.set("mcc", mcc as i64);
            position.set("mnc", mnc as i64);
            position.set("lac", lac as i64);
            position.set("cellId", cell_id as i64);
        }

        // Parse status info if present
        if has_status(msg_type) && pos + 1 <= buf.len() {
            let device_info = buf[pos];
            position.set(position_keys::KEY_STATUS, device_info as i64);

            // Bit 1: ACC/ignition
            let ignition = (device_info & 0x02) != 0;
            position.set(position_keys::KEY_IGNITION, ignition);
            pos += 1;

            // Voltage level (1 byte) and GSM signal (1 byte)
            if pos + 2 <= buf.len() {
                let voltage_level = buf[pos];
                let gsm_signal = buf[pos + 1];
                position.set(position_keys::KEY_POWER, voltage_level as f64);
                position.set(position_keys::KEY_RSSI, gsm_signal as i64);
                pos += 2;
            }

            // Alarm type (1 byte)
            if pos < buf.len() {
                let alarm_byte = buf[pos];
                if let Some(alarm) = decode_alarm_byte(alarm_byte) {
                    position.add_alarm(alarm);
                }
            }
        }

        Ok(Some(vec![position]))
    }
}

// ─── Protocol encoder ───────────────────────────────────────────────

pub struct Gt06Encoder;

impl ProtocolEncoder for Gt06Encoder {
    fn encode(&self, command: &Command, _unique_id: &str) -> ProtocolResult<Bytes> {
        let content = match command.command_type.as_str() {
            Command::TYPE_ENGINE_STOP => "Relay,1#",
            Command::TYPE_ENGINE_RESUME => "Relay,0#",
            Command::TYPE_CUSTOM => {
                let data = command
                    .get_string(Command::KEY_DATA)
                    .ok_or_else(|| ProtocolError::Encode("Missing data attribute".to_string()))?;
                return self.build_command_packet(data.as_bytes());
            }
            other => {
                return Err(ProtocolError::UnsupportedCommand(other.to_string()));
            }
        };

        self.build_command_packet(content.as_bytes())
    }

    fn supported_commands(&self) -> &[&str] {
        &[
            Command::TYPE_ENGINE_STOP,
            Command::TYPE_ENGINE_RESUME,
            Command::TYPE_CUSTOM,
        ]
    }
}

impl Gt06Encoder {
    fn build_command_packet(&self, content: &[u8]) -> ProtocolResult<Bytes> {
        let content_len = content.len();
        // Packet body: protocol(1) + server_flag(4) + content_length(1) + content + serial(2) + crc(2)
        let body_len = 1 + 4 + 1 + content_len + 2 + 2;

        let mut buf = BytesMut::with_capacity(2 + 1 + body_len + 2);

        // Start marker
        buf.put_u8(0x78);
        buf.put_u8(0x78);

        // Packet length (protocol number through CRC)
        buf.put_u8(body_len as u8);

        // Protocol number (server command)
        buf.put_u8(MSG_COMMAND_0);

        // Server flag bit (4 bytes, set to 0)
        buf.put_u32(0);

        // Command content length + content
        buf.put_u8(content_len as u8);
        buf.put_slice(content);

        // Serial number
        buf.put_u16(0x0001);

        // CRC-16 X.25 over data from length byte to serial (inclusive)
        let crc_start = 2; // after start marker
        let crc_end = buf.len();
        let crc = crc16_x25(&buf[crc_start..crc_end]);
        buf.put_u16(crc);

        // End marker
        buf.put_u8(0x0D);
        buf.put_u8(0x0A);

        Ok(buf.freeze())
    }
}

// ─── Registration ───────────────────────────────────────────────────

pub fn register(registry: &mut ProtocolRegistry) {
    let encoder = Gt06Encoder;
    let supported = encoder
        .supported_commands()
        .iter()
        .map(|s| s.to_string())
        .collect();

    registry.register(ProtocolDefinition {
        name: PROTOCOL_NAME.to_string(),
        transport: Transport::Tcp,
        default_port: DEFAULT_PORT,
        decoder_factory: Box::new(|| Box::new(Gt06Decoder)),
        encoder_factory: Box::new(|| Box::new(Gt06Encoder)),
        frame_decoder_factory: Box::new(|| Box::new(Gt06FrameDecoder)),
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

    #[test]
    fn test_gt06_frame_decoder() {
        let mut decoder = Gt06FrameDecoder;
        let mut buf = BytesMut::new();

        // Construct a minimal GT06 login packet
        buf.put_u8(0x78);
        buf.put_u8(0x78);
        buf.put_u8(0x0D); // length: 13 bytes (proto + 8 imei + 2 serial + 2 crc)
        buf.put_u8(MSG_LOGIN);
        buf.put_slice(&[0x03, 0x59, 0x58, 0x60, 0x15, 0x82, 0x98, 0x02]);
        buf.put_u16(0x0001);
        buf.put_u16(0x0000);
        buf.put_u8(0x0D);
        buf.put_u8(0x0A);

        use tokio_util::codec::Decoder;
        let frame = decoder.decode(&mut buf).unwrap();
        assert!(frame.is_some());
        let frame = frame.unwrap();
        assert_eq!(frame[0], 0x78);
        assert_eq!(frame[1], 0x78);
        assert_eq!(frame[frame.len() - 2], 0x0D);
        assert_eq!(frame[frame.len() - 1], 0x0A);
    }

    #[tokio::test]
    async fn test_decode_login() {
        let decoder = Gt06Decoder;
        let mut session = DeviceSession::unauthenticated(PROTOCOL_NAME);

        let mut buf = BytesMut::new();
        buf.put_u8(0x78);
        buf.put_u8(0x78);
        buf.put_u8(0x0D);
        buf.put_u8(MSG_LOGIN);
        buf.put_slice(&[0x03, 0x59, 0x58, 0x60, 0x15, 0x82, 0x98, 0x02]);
        buf.put_u16(0x0001);
        buf.put_u16(0x0000);
        buf.put_u8(0x0D);
        buf.put_u8(0x0A);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none()); // login returns no positions
        assert!(!session.unique_id.is_empty());
        assert!(session.device_id != 0);
    }

    #[tokio::test]
    async fn test_decode_location() {
        let decoder = Gt06Decoder;
        let mut session = DeviceSession::new(
            simple_device_id("359586015829802"),
            "359586015829802".to_string(),
            PROTOCOL_NAME.to_string(),
        );

        // Build a GPS message
        let mut buf = BytesMut::new();
        buf.put_u8(0x78);
        buf.put_u8(0x78);
        buf.put_u8(0x1F); // length
        buf.put_u8(MSG_GPS_LBS_1); // protocol number

        // Date: 2024-03-15 10:30:45
        buf.put_u8(24); // year
        buf.put_u8(3);  // month
        buf.put_u8(15); // day
        buf.put_u8(10); // hour
        buf.put_u8(30); // minute
        buf.put_u8(45); // second

        // GPS info: length=12, satellites=8
        buf.put_u8(0xC8);

        // Latitude: 22.5 degrees = 22.5 * 60 * 30000 = 40500000
        buf.put_u32(40500000);

        // Longitude: 114.0 degrees = 114.0 * 60 * 30000 = 205200000
        buf.put_u32(205200000);

        // Speed: 60 km/h
        buf.put_u8(60);

        // Course + flags: real-time, positioned, north, east
        // 0x3000 | course 180 = 0x30B4
        buf.put_u16(0x30B4);

        // LBS data (8 bytes)
        buf.put_u16(460);  // MCC
        buf.put_u8(0);     // MNC
        buf.put_u16(1234); // LAC
        buf.put_u8(0);     // CellID high
        buf.put_u16(5678); // CellID low

        // Padding to match length
        buf.put_u16(0x0001); // serial
        buf.put_u16(0x0000); // crc
        buf.put_u8(0x0D);
        buf.put_u8(0x0A);

        let result = decoder.decode(&mut buf, &mut session).await;
        assert!(result.is_ok());
        let positions = result.unwrap();
        assert!(positions.is_some());
        let positions = positions.unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[0].protocol, PROTOCOL_NAME);
        assert!(positions[0].valid);
    }

    #[test]
    fn test_encode_engine_stop() {
        let encoder = Gt06Encoder;
        let command = Command {
            id: 0,
            device_id: 1,
            command_type: Command::TYPE_ENGINE_STOP.to_string(),
            description: None,
            attributes: serde_json::Value::Object(serde_json::Map::new()),
        };

        let result = encoder.encode(&command, "test");
        assert!(result.is_ok());
        let bytes = result.unwrap();
        // Verify start marker
        assert_eq!(bytes[0], 0x78);
        assert_eq!(bytes[1], 0x78);
        // Verify protocol number
        assert_eq!(bytes[3], MSG_COMMAND_0);
        // Verify end marker
        assert_eq!(bytes[bytes.len() - 2], 0x0D);
        assert_eq!(bytes[bytes.len() - 1], 0x0A);
    }

    #[test]
    fn test_alarm_decode() {
        assert_eq!(
            decode_alarm_byte(0x01),
            Some(traccar_model::alarm::SOS)
        );
        assert_eq!(
            decode_alarm_byte(0x06),
            Some(traccar_model::alarm::OVERSPEED)
        );
        assert_eq!(decode_alarm_byte(0xFF), None);
    }
}
