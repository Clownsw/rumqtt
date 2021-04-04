use super::*;
use bytes::{Buf, BufMut, Bytes, BytesMut};

/// Return code in connack
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum PubRelReason {
    Success = 0,
    PacketIdentifierNotFound = 146,
}

/// Acknowledgement to QoS1 publish
#[derive(Debug, Clone, PartialEq)]
pub struct PubRel {
    pub pkid: u16,
    pub reason: PubRelReason,
    pub properties: PubRelProperties,
}

impl PubRel {
    pub fn new(pkid: u16) -> PubRel {
        PubRel {
            pkid,
            reason: PubRelReason::Success,
            properties: PubRelProperties::default(),
        }
    }

    fn len(&self) -> usize {
        let mut len = 2 + 1; // pkid + reason

        // If there are no properties during success, sending reason code is optional
        if self.reason == PubRelReason::Success && self.properties.len() == 0 {
            return 2;
        }

        let properties_len = self.properties.len();
        let properties_len_len = len_len(properties_len);
        len += properties_len_len + properties_len;

        len
    }

    pub fn read(fixed_header: FixedHeader, mut bytes: Bytes) -> Result<Self, Error> {
        let variable_header_index = fixed_header.fixed_header_len;
        bytes.advance(variable_header_index);
        let pkid = read_u16(&mut bytes)?;
        if fixed_header.remaining_len == 2 {
            return Ok(PubRel {
                pkid,
                reason: PubRelReason::Success,
                properties: PubRelProperties::default(),
            });
        }

        let ack_reason = read_u8(&mut bytes)?;
        if fixed_header.remaining_len < 4 {
            return Ok(PubRel {
                pkid,
                reason: reason(ack_reason)?,
                properties: PubRelProperties::default(),
            });
        }

        let puback = PubRel {
            pkid,
            reason: reason(ack_reason)?,
            properties: PubRelProperties::extract(&mut bytes)?,
        };

        Ok(puback)
    }

    pub fn write(&self, buffer: &mut BytesMut) -> Result<usize, Error> {
        let len = self.len();
        buffer.put_u8(0x62);
        let count = write_remaining_length(buffer, len)?;
        buffer.put_u16(self.pkid);

        // If there are no properties during success, sending reason code is optional
        if self.reason == PubRelReason::Success && self.properties.len() == 0 {
            return Ok(4);
        }

        buffer.put_u8(self.reason as u8);

        self.properties.write(buffer)?;
        Ok(1 + count + len)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PubRelProperties {
    pub reason_string: Option<String>,
    pub user_properties: Vec<(String, String)>,
}

impl PubRelProperties {
    pub fn len(&self) -> usize {
        let mut len = 0;

        if let Some(reason) = &self.reason_string {
            len += 1 + 2 + reason.len();
        }

        for (key, value) in self.user_properties.iter() {
            len += 1 + 2 + key.len() + 2 + value.len();
        }

        len
    }

    pub fn extract(mut bytes: &mut Bytes) -> Result<PubRelProperties, Error> {
        let mut reason_string = None;
        let mut user_properties = Vec::new();

        let (properties_len_len, properties_len) = length(bytes.iter())?;
        bytes.advance(properties_len_len);
        if properties_len == 0 {
            return Ok(PubRelProperties::default());
        }

        let mut cursor = 0;
        // read until cursor reaches property length. properties_len = 0 will skip this loop
        while cursor < properties_len {
            let prop = read_u8(&mut bytes)?;
            cursor += 1;

            match property(prop)? {
                PropertyType::ReasonString => {
                    let reason = read_mqtt_string(&mut bytes)?;
                    cursor += 2 + reason.len();
                    reason_string = Some(reason);
                }
                PropertyType::UserProperty => {
                    let key = read_mqtt_string(&mut bytes)?;
                    let value = read_mqtt_string(&mut bytes)?;
                    cursor += 2 + key.len() + 2 + value.len();
                    user_properties.push((key, value));
                }
                _ => return Err(Error::InvalidPropertyType(prop)),
            }
        }

        Ok(PubRelProperties {
            reason_string,
            user_properties,
        })
    }

    fn write(&self, buffer: &mut BytesMut) -> Result<(), Error> {
        let len = self.len();
        write_remaining_length(buffer, len)?;

        if let Some(reason) = &self.reason_string {
            buffer.put_u8(PropertyType::ReasonString as u8);
            write_mqtt_string(buffer, reason);
        }

        for (key, value) in self.user_properties.iter() {
            buffer.put_u8(PropertyType::UserProperty as u8);
            write_mqtt_string(buffer, key);
            write_mqtt_string(buffer, value);
        }

        Ok(())
    }
}
/// Connection return code type
fn reason(num: u8) -> Result<PubRelReason, Error> {
    let code = match num {
        0 => PubRelReason::Success,
        146 => PubRelReason::PacketIdentifierNotFound,
        num => return Err(Error::InvalidConnectReturnCode(num)),
    };

    Ok(code)
}

#[cfg(test)]
mod test {
    use super::*;
    use alloc::vec;
    use bytes::BytesMut;
    use pretty_assertions::assert_eq;

    fn sample() -> PubRel {
        let properties = PubRelProperties {
            reason_string: Some("test".to_owned()),
            user_properties: vec![("test".to_owned(), "test".to_owned())],
        };

        PubRel {
            pkid: 42,
            reason: PubRelReason::PacketIdentifierNotFound,
            properties,
        }
    }

    fn sample_bytes() -> Vec<u8> {
        vec![
            0x62, // payload type
            0x18, // remaining length
            0x00, 0x2a, // packet id
            0x92, // reason
            0x14, // properties len
            0x1f, 0x00, 0x04, 0x74, 0x65, 0x73, 0x74, // reason_string
            0x26, 0x00, 0x04, 0x74, 0x65, 0x73, 0x74, 0x00, 0x04, 0x74, 0x65, 0x73,
            0x74, // user properties
        ]
    }

    #[test]
    fn pubrel_parsing_works() {
        let mut stream = bytes::BytesMut::new();
        let packetstream = &sample_bytes();
        stream.extend_from_slice(&packetstream[..]);

        let fixed_header = parse_fixed_header(stream.iter()).unwrap();
        let pubrel_bytes = stream.split_to(fixed_header.frame_length()).freeze();
        let pubrel = PubRel::read(fixed_header, pubrel_bytes).unwrap();
        assert_eq!(pubrel, sample());
    }

    #[test]
    fn pubrel_encoding_works() {
        let pubrel = sample();
        let mut buf = BytesMut::new();
        pubrel.write(&mut buf).unwrap();
        assert_eq!(&buf[..], sample_bytes());
    }
}
