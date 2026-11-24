//! 帧结构发送：帧头 + 数据 + 校验 + 帧尾（对标 sscom）。
//!
//! 校验范围：仅载荷 / 帧头+载荷 / 帧头+载荷（帧尾除外）——用 `CrcRange` 表达。

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::hexutil::parse_hex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CrcAlgo {
    #[default]
    None,
    /// 累加和（低 8 位）
    Sum8,
    /// 逐字节异或
    Xor,
    /// LRC：(-sum) & 0xFF
    Lrc,
    /// CRC-16/MODBUS（poly 0xA001，init 0xFFFF，结果小端）
    Crc16Modbus,
    /// CRC-16/CCITT-FALSE（poly 0x1021，init 0xFFFF，结果大端）
    Crc16Ccitt,
    /// CRC-16/XMODEM（poly 0x1021，init 0x0000，结果大端）
    Crc16Xmodem,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CrcRange {
    /// 仅载荷
    #[default]
    Payload,
    /// 帧头 + 载荷
    HeaderPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameSpec {
    /// 帧头（Hex 字符串，如 "AA 55"）
    pub header: String,
    pub crc: CrcAlgo,
    pub crc_range: CrcRange,
    /// 帧尾（Hex 字符串）
    pub tail: String,
}

impl Default for FrameSpec {
    fn default() -> Self {
        Self {
            header: String::new(),
            crc: CrcAlgo::None,
            crc_range: CrcRange::Payload,
            tail: String::new(),
        }
    }
}

impl FrameSpec {
    /// 组帧。payload 为已编码的载荷字节。
    pub fn build(&self, payload: &[u8]) -> Result<Vec<u8>> {
        let header = parse_hex(&self.header)?;
        let tail = parse_hex(&self.tail)?;

        let crc_bytes: Vec<u8> = match self.crc {
            CrcAlgo::None => Vec::new(),
            algo => {
                let target: Vec<u8> = match self.crc_range {
                    CrcRange::Payload => payload.to_vec(),
                    CrcRange::HeaderPayload => header.iter().chain(payload.iter()).copied().collect(),
                };
                let word = checksum16(algo, &target);
                match algo {
                    CrcAlgo::Sum8 | CrcAlgo::Xor | CrcAlgo::Lrc => vec![word as u8],
                    // MODBUS 小端；CCITT/XMODEM 大端
                    CrcAlgo::Crc16Modbus => vec![(word & 0xFF) as u8, (word >> 8) as u8],
                    _ => vec![(word >> 8) as u8, (word & 0xFF) as u8],
                }
            }
        };

        let mut out = Vec::with_capacity(header.len() + payload.len() + crc_bytes.len() + tail.len());
        out.extend(header);
        out.extend(payload);
        out.extend(crc_bytes);
        out.extend(tail);
        Ok(out)
    }
}

/// 统一返回 16 位字：8 位算法只占低 8 位。
pub fn checksum16(algo: CrcAlgo, data: &[u8]) -> u16 {
    match algo {
        CrcAlgo::None => 0,
        CrcAlgo::Sum8 => data.iter().fold(0u16, |a, &b| a.wrapping_add(b as u16)) & 0xFF,
        CrcAlgo::Xor => data.iter().fold(0u8, |a, &b| a ^ b) as u16,
        CrcAlgo::Lrc => 0u16.wrapping_sub(data.iter().fold(0u16, |a, &b| a.wrapping_add(b as u16))) & 0xFF,
        CrcAlgo::Crc16Modbus => crc16_reflected(data, 0xFFFF),
        CrcAlgo::Crc16Ccitt => crc16_direct(data, 0xFFFF),
        CrcAlgo::Crc16Xmodem => crc16_direct(data, 0x0000),
    }
}

/// 反射式 CRC-16（MODBUS，poly 0xA001）。
fn crc16_reflected(data: &[u8], init: u16) -> u16 {
    let mut crc = init;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

/// 直接式 CRC-16（poly 0x1021，MSB first）：CCITT-FALSE / XMODEM。
fn crc16_direct(data: &[u8], init: u16) -> u16 {
    let mut crc = init;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

/// 便捷入口：从 UI 文本直接组帧。`hex_payload=true` 时 payload 按 Hex 解析，
/// 否则按转义符展开。
pub fn build_from_text(
    spec: &FrameSpec,
    text: &str,
    hex_payload: bool,
) -> Result<Vec<u8>> {
    let payload = if hex_payload {
        parse_hex(text)?
    } else {
        crate::hexutil::unescape(text)
    };
    if payload.is_empty() && parse_hex(&spec.header).map(|h| h.is_empty()).unwrap_or(true) {
        return Err(Error::InvalidHex("载荷与帧头均为空".into()));
    }
    spec.build(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 标准校验向量（数据 "123456789"）
    const CHECK: &[u8] = b"123456789";

    #[test]
    fn crc16_vectors() {
        assert_eq!(checksum16(CrcAlgo::Crc16Modbus, CHECK), 0x4B37);
        assert_eq!(checksum16(CrcAlgo::Crc16Ccitt, CHECK), 0x29B1);
        assert_eq!(checksum16(CrcAlgo::Crc16Xmodem, CHECK), 0x31C3);
    }

    #[test]
    fn sum_xor_lrc_vectors() {
        assert_eq!(checksum16(CrcAlgo::Sum8, CHECK), 0xDD);
        assert_eq!(checksum16(CrcAlgo::Xor, CHECK), 0x31);
        assert_eq!(checksum16(CrcAlgo::Lrc, CHECK), 0x23);
    }

    #[test]
    fn frame_layout() {
        let spec = FrameSpec {
            header: "AA 55".into(),
            crc: CrcAlgo::Sum8,
            crc_range: CrcRange::HeaderPayload,
            tail: "0D 0A".into(),
        };
        let out = spec.build(&[0x01, 0x02]).unwrap();
        // sum(AA 55 01 02) = 0x102 & 0xFF = 0x02
        assert_eq!(out, vec![0xAA, 0x55, 0x01, 0x02, 0x02, 0x0D, 0x0A]);
    }

    #[test]
    fn modbus_is_little_endian() {
        let spec = FrameSpec {
            header: String::new(),
            crc: CrcAlgo::Crc16Modbus,
            crc_range: CrcRange::Payload,
            tail: String::new(),
        };
        let out = spec.build(b"123456789").unwrap();
        // 载荷 + MODBUS 小端 CRC
        assert_eq!(out, [b"123456789".to_vec(), vec![0x37, 0x4B]].concat());
    }

    #[test]
    fn ccitt_is_big_endian() {
        let spec = FrameSpec {
            header: String::new(),
            crc: CrcAlgo::Crc16Ccitt,
            crc_range: CrcRange::Payload,
            tail: String::new(),
        };
        let out = spec.build(b"123456789").unwrap();
        // 载荷 + CCITT 大端 CRC
        assert_eq!(out, [b"123456789".to_vec(), vec![0x29, 0xB1]].concat());
    }

    #[test]
    fn payload_from_hex_text() {
        let spec = FrameSpec {
            header: "A5".into(),
            crc: CrcAlgo::Xor,
            crc_range: CrcRange::Payload,
            tail: String::new(),
        };
        let out = build_from_text(&spec, "AA 55", true).unwrap();
        assert_eq!(out, vec![0xA5, 0xAA, 0x55, (0xAA ^ 0x55) as u8]);
    }

    #[test]
    fn payload_from_escaped_text() {
        let spec = FrameSpec::default();
        let out = build_from_text(&spec, "AT\\r\\n", false).unwrap();
        assert_eq!(out, b"AT\r\n".to_vec());
    }

    #[test]
    fn invalid_header_hex_errors() {
        let spec = FrameSpec {
            header: "ZZ".into(),
            crc: CrcAlgo::None,
            crc_range: CrcRange::Payload,
            tail: String::new(),
        };
        assert!(spec.build(&[1]).is_err());
    }
}
