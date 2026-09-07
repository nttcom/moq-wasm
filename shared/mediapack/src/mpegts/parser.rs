use anyhow::{Result, ensure};
use bytes::{Bytes, BytesMut};

pub const PACKET_SIZE: usize = 188;
pub const SYNC_BYTE: u8 = 0x47;
pub const PAT_PID: u16 = 0x0000;
pub const CLOCK_RATE: u32 = 90_000;
pub const STREAM_TYPE_AAC_ADTS: u8 = 0x0F;
pub const STREAM_TYPE_H264: u8 = 0x1B;

#[derive(Debug, PartialEq, Eq)]
pub struct TsPacket<'a> {
    pub pid: u16,
    pub payload_unit_start: bool,
    pub transport_error: bool,
    pub continuity_counter: u8,
    pub payload: &'a [u8],
}

pub fn parse_packet(packet: &[u8]) -> Result<TsPacket<'_>> {
    ensure!(
        packet.len() == PACKET_SIZE,
        "TS packet must be {PACKET_SIZE} bytes, got {}",
        packet.len()
    );
    ensure!(packet[0] == SYNC_BYTE, "TS packet missing sync byte");
    let adaptation_field_control = (packet[3] >> 4) & 0b11;
    let mut payload_start = 4;
    if adaptation_field_control & 0b10 != 0 {
        payload_start += 1 + packet[4] as usize;
        ensure!(
            payload_start <= PACKET_SIZE,
            "TS adaptation field overflows packet"
        );
    }
    let payload = if adaptation_field_control & 0b01 != 0 {
        &packet[payload_start..]
    } else {
        &[]
    };
    Ok(TsPacket {
        pid: u16::from_be_bytes([packet[1] & 0x1F, packet[2]]),
        payload_unit_start: packet[1] & 0x40 != 0,
        transport_error: packet[1] & 0x80 != 0,
        continuity_counter: packet[3] & 0x0F,
        payload,
    })
}

#[derive(Default)]
pub struct PacketReader {
    buffer: BytesMut,
}

impl PacketReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) -> Vec<Bytes> {
        self.buffer.extend_from_slice(data);
        let mut packets = Vec::new();
        while self.buffer.len() >= PACKET_SIZE {
            if !self.is_aligned() {
                if !self.resynchronize() {
                    break;
                }
                continue;
            }
            packets.push(self.buffer.split_to(PACKET_SIZE).freeze());
        }
        packets
    }

    fn is_aligned(&self) -> bool {
        self.buffer[0] == SYNC_BYTE
            && self
                .buffer
                .get(PACKET_SIZE)
                .is_none_or(|next| *next == SYNC_BYTE)
    }

    fn resynchronize(&mut self) -> bool {
        for candidate in 1..self.buffer.len() {
            if self.buffer[candidate] != SYNC_BYTE {
                continue;
            }
            let next_sync = candidate + PACKET_SIZE;
            if next_sync >= self.buffer.len() {
                let _ = self.buffer.split_to(candidate);
                return false;
            }
            if self.buffer[next_sync] == SYNC_BYTE {
                let _ = self.buffer.split_to(candidate);
                return true;
            }
        }
        self.buffer.clear();
        false
    }
}

const SECTION_HEADER_LENGTH: usize = 8;
const CRC_LENGTH: usize = 4;

#[derive(Debug, PartialEq, Eq)]
pub struct Section<'a> {
    pub table_id: u8,
    pub body: &'a [u8],
}

pub fn parse_section(data: &[u8]) -> Result<Option<Section<'_>>> {
    if data.len() < 3 {
        return Ok(None);
    }
    let section_length = u16::from_be_bytes([data[1] & 0x0F, data[2]]) as usize;
    let total_length = 3 + section_length;
    if data.len() < total_length {
        return Ok(None);
    }
    ensure!(
        total_length >= SECTION_HEADER_LENGTH + CRC_LENGTH,
        "PSI section too short: {total_length} bytes"
    );
    Ok(Some(Section {
        table_id: data[0],
        body: &data[SECTION_HEADER_LENGTH..total_length - CRC_LENGTH],
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Program {
    pub number: u16,
    pub pmt_pid: u16,
}

pub fn parse_pat(body: &[u8]) -> Vec<Program> {
    body.chunks_exact(4)
        .map(|entry| Program {
            number: u16::from_be_bytes([entry[0], entry[1]]),
            pmt_pid: u16::from_be_bytes([entry[2] & 0x1F, entry[3]]),
        })
        .filter(|program| program.number != 0)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementaryStream {
    pub pid: u16,
    pub stream_type: u8,
}

pub fn parse_pmt(body: &[u8]) -> Result<Vec<ElementaryStream>> {
    ensure!(body.len() >= 4, "PMT body too short");
    let program_info_length = u16::from_be_bytes([body[2] & 0x0F, body[3]]) as usize;
    let mut offset = 4 + program_info_length;
    let mut streams = Vec::new();
    while offset + 5 <= body.len() {
        let es_info_length =
            u16::from_be_bytes([body[offset + 3] & 0x0F, body[offset + 4]]) as usize;
        streams.push(ElementaryStream {
            pid: u16::from_be_bytes([body[offset + 1] & 0x1F, body[offset + 2]]),
            stream_type: body[offset],
        });
        offset += 5 + es_info_length;
    }
    Ok(streams)
}

#[derive(Debug, PartialEq, Eq)]
pub struct PesHeader {
    pub stream_id: u8,
    pub pts: Option<u64>,
    pub dts: Option<u64>,
    pub header_length: usize,
}

pub fn parse_pes_header(data: &[u8]) -> Result<Option<PesHeader>> {
    if data.len() < 9 {
        return Ok(None);
    }
    ensure!(
        data[..3] == [0, 0, 1],
        "PES packet missing start code prefix"
    );
    ensure!(data[6] & 0xC0 == 0x80, "PES packet missing marker bits");
    let pts_dts_flags = data[7] >> 6;
    let header_length = 9 + data[8] as usize;
    if data.len() < header_length {
        return Ok(None);
    }
    let pts = (pts_dts_flags & 0b10 != 0).then(|| read_timestamp(&data[9..14]));
    let dts = (pts_dts_flags == 0b11).then(|| read_timestamp(&data[14..19]));
    Ok(Some(PesHeader {
        stream_id: data[3],
        pts,
        dts,
        header_length,
    }))
}

fn read_timestamp(bytes: &[u8]) -> u64 {
    ((bytes[0] as u64 >> 1) & 0x07) << 30
        | (bytes[1] as u64) << 22
        | (bytes[2] as u64 >> 1) << 15
        | (bytes[3] as u64) << 7
        | (bytes[4] as u64 >> 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{pat_section, pes_packet, pmt_section, ts_packet};

    #[test]
    fn parses_packet_with_adaptation_field() {
        // Arrange
        let packet = ts_packet(0x100, true, 5, &[1, 2, 3]);

        // Act
        let parsed = parse_packet(&packet).unwrap();

        // Assert
        assert_eq!(parsed.pid, 0x100);
        assert!(parsed.payload_unit_start);
        assert_eq!(parsed.continuity_counter, 5);
        assert_eq!(parsed.payload, [1, 2, 3]);
    }

    #[test]
    fn reader_resynchronizes_on_sync_byte_followed_by_another_packet() {
        // Arrange
        let mut stream = vec![0x47, 0x11, 0x22];
        stream.extend(ts_packet(0x100, false, 0, &[9]));
        stream.extend(ts_packet(0x100, false, 1, &[8]));
        let mut reader = PacketReader::new();

        // Act
        let packets = reader.push(&stream);

        // Assert
        assert_eq!(packets.len(), 2);
        assert_eq!(parse_packet(&packets[0]).unwrap().payload, [9]);
        assert_eq!(parse_packet(&packets[1]).unwrap().payload, [8]);
    }

    #[test]
    fn parses_pat_and_pmt_sections() {
        // Arrange
        let pat = pat_section(0x1000);
        let pmt = pmt_section(&[(0x100, STREAM_TYPE_H264), (0x101, STREAM_TYPE_AAC_ADTS)]);

        // Act
        let programs = parse_pat(parse_section(&pat[1..]).unwrap().unwrap().body);
        let streams = parse_pmt(parse_section(&pmt[1..]).unwrap().unwrap().body).unwrap();

        // Assert
        assert_eq!(
            programs,
            [Program {
                number: 1,
                pmt_pid: 0x1000
            }]
        );
        assert_eq!(
            streams,
            [
                ElementaryStream {
                    pid: 0x100,
                    stream_type: STREAM_TYPE_H264
                },
                ElementaryStream {
                    pid: 0x101,
                    stream_type: STREAM_TYPE_AAC_ADTS
                }
            ]
        );
    }

    #[test]
    fn reports_incomplete_section() {
        // Arrange
        let pmt = pmt_section(&[(0x100, STREAM_TYPE_H264)]);

        // Act
        let section = parse_section(&pmt[1..pmt.len() - 1]).unwrap();

        // Assert
        assert!(section.is_none());
    }

    #[test]
    fn parses_pes_header_with_pts_and_dts() {
        // Arrange
        let pes = pes_packet(0xE0, 0x1_2345_6789, Some(0x1_2345_6700), &[0xAA]);

        // Act
        let header = parse_pes_header(&pes).unwrap().unwrap();

        // Assert
        assert_eq!(header.stream_id, 0xE0);
        assert_eq!(header.pts, Some(0x1_2345_6789));
        assert_eq!(header.dts, Some(0x1_2345_6700));
        assert_eq!(&pes[header.header_length..], [0xAA]);
    }

    #[test]
    fn parses_pes_header_with_pts_only() {
        // Arrange
        let pes = pes_packet(0xC0, 90_000, None, &[]);

        // Act
        let header = parse_pes_header(&pes).unwrap().unwrap();

        // Assert
        assert_eq!(header.pts, Some(90_000));
        assert_eq!(header.dts, None);
        assert_eq!(header.header_length, pes.len());
    }
}
