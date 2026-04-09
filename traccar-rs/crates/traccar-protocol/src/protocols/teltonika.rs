use bytes::{Buf, Bytes, BytesMut};
use chrono::Utc;
use traccar_model::{Command, Position};
use crate::{ProtocolDecoder, ProtocolEncoder, ProtocolError, ProtocolRegistry, ProtocolDefinition, Transport};
use crate::codec::LengthFieldFrameDecoder;
use crate::session::DeviceSession;
use async_trait::async_trait;
use traccar_helper::date;

pub struct TeltonikaDecoder;

#[async_trait]
impl ProtocolDecoder for TeltonikaDecoder {
    async fn decode(
        &self,
        buf: &mut BytesMut,
        session: &mut DeviceSession,
    ) -> Result<Option<Vec<Position>>, ProtocolError> {
        if buf.len() < 4 {
            return Ok(None);
        }

        let mut reader = buf.clone();

        // Check if this is IMEI packet (first 2 bytes = IMEI length)
        let first_two = reader.get_u16();
        if first_two > 0 && first_two < 20 {
            // IMEI login packet
            let imei_len = first_two as usize;
            if reader.remaining() < imei_len {
                return Ok(None);
            }
            let imei_bytes = reader.copy_to_bytes(imei_len);
            session.unique_id = String::from_utf8_lossy(&imei_bytes).to_string();
            tracing::info!("Teltonika login: {}", session.unique_id);
            return Ok(None);
        }

        // Data packet: preamble(4) + data_length(4) + codec_id(1) + count(1) + AVL data + count(1) + crc(4)
        reader = buf.clone();
        let preamble = reader.get_u32();
        if preamble != 0 {
            return Err(ProtocolError::InvalidData("Invalid Teltonika preamble".into()));
        }

        let data_length = reader.get_u32() as usize;
        if reader.remaining() < data_length + 4 {
            return Ok(None); // Wait for more data
        }

        let codec_id = reader.get_u8();
        let count = reader.get_u8() as usize;

        let mut positions = Vec::with_capacity(count);

        for _ in 0..count {
            if reader.remaining() < 24 {
                break;
            }

            let mut position = Position::new("teltonika");
            position.device_id = session.device_id;
            position.server_time = Utc::now();

            // Timestamp (8 bytes, milliseconds since epoch)
            let timestamp = reader.get_u64() as i64;
            position.set_time(date::from_unix_millis(timestamp));

            // Priority
            let _priority = reader.get_u8();

            // Longitude (4 bytes, signed, / 10000000.0)
            let raw_lon = reader.get_i32();
            position.longitude = raw_lon as f64 / 10_000_000.0;

            // Latitude (4 bytes, signed, / 10000000.0)
            let raw_lat = reader.get_i32();
            position.latitude = raw_lat as f64 / 10_000_000.0;

            // Altitude (2 bytes)
            position.altitude = reader.get_i16() as f64;

            // Angle/Course (2 bytes)
            position.course = reader.get_u16() as f64;

            // Satellites (1 byte)
            let satellites = reader.get_u8();
            position.set("sat", satellites as i64);

            // Speed (2 bytes, km/h)
            let speed_kmh = reader.get_u16() as f64;
            position.speed = traccar_helper::knots_from_kph(speed_kmh);

            position.valid = satellites > 0;

            // Skip IO data (codec 8 or 8 extended)
            if codec_id == 8 || codec_id == 0x8E {
                // Event IO ID
                if codec_id == 0x8E {
                    let _event_id = reader.get_u16();
                    let _total_count = reader.get_u16();
                } else {
                    let _event_id = reader.get_u8();
                    let _total_count = reader.get_u8();
                }

                // IO elements: 1-byte, 2-byte, 4-byte, 8-byte values
                for value_size in [1u8, 2, 4, 8] {
                    let io_count = if codec_id == 0x8E {
                        reader.get_u16() as usize
                    } else {
                        reader.get_u8() as usize
                    };
                    for _ in 0..io_count {
                        let _io_id = if codec_id == 0x8E {
                            reader.get_u16() as u32
                        } else {
                            reader.get_u8() as u32
                        };
                        reader.advance(value_size as usize);
                    }
                }
            }

            positions.push(position);
        }

        if positions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(positions))
        }
    }
}

pub struct TeltonikaEncoder;

impl ProtocolEncoder for TeltonikaEncoder {
    fn encode(&self, _command: &Command, _unique_id: &str) -> Result<Bytes, ProtocolError> {
        Err(ProtocolError::UnsupportedCommand("Teltonika commands not yet implemented".into()))
    }
}

pub fn register(registry: &mut ProtocolRegistry) {
    registry.register(ProtocolDefinition {
        name: "teltonika".to_string(),
        transport: Transport::Tcp,
        default_port: 5027,
        decoder_factory: Box::new(|| Box::new(TeltonikaDecoder)),
        encoder_factory: Box::new(|| Box::new(TeltonikaEncoder)),
        frame_decoder_factory: Box::new(|| {
            Box::new(LengthFieldFrameDecoder::new(8192, 4, 4, 4, 0))
        }),
        supported_commands: vec![],
    });
}
