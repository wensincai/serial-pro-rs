//! Hex 解析 / 格式化 / 控制字符可视化。

use crate::error::{Error, Result};

/// 解析 Hex 字符串：容忍空格、逗号、0x 前缀、大小写。
/// 例："AA 55 0x1b,7f" -> [0xAA, 0x55, 0x1B, 0x7F]
pub fn parse_hex(s: &str) -> Result<Vec<u8>> {
    let cleaned: String = s
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n' | ','))
        .collect();
    let cleaned = cleaned
        .to_lowercase()
        .replace("0x", "")
        .replace("\\x", "");
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if !cleaned.len().is_multiple_of(2) {
        return Err(Error::InvalidHex(format!("长度为奇数: {s:?}")));
    }
    let mut out = Vec::with_capacity(cleaned.len() / 2);
    let bytes = cleaned.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_digit(bytes[i] as char)
            .ok_or_else(|| Error::InvalidHex(format!("非法字符 '{}' 于 {s:?}", bytes[i] as char)))?;
        let lo = hex_digit(bytes[i + 1] as char).ok_or_else(|| {
            Error::InvalidHex(format!("非法字符 '{}' 于 {:?}", bytes[i + 1] as char, s))
        })?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_digit(c: char) -> Option<u8> {
    match c {
        '0'..='9' => Some(c as u8 - b'0'),
        'a'..='f' => Some(c as u8 - b'a' + 10),
        _ => None,
    }
}

/// 字节转 Hex 字符串，`sep` 可为空串。
pub fn to_hex(bytes: &[u8], sep: &str) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(sep)
}

/// 展开转义符：`\n \r \t \\ \0 \xNN`。
pub fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            continue;
        }
        match chars.next() {
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some('0') => out.push(0),
            Some('x') => {
                let h = chars.next().and_then(|c| c.to_digit(16));
                let l = chars.next().and_then(|c| c.to_digit(16));
                match (h, l) {
                    (Some(h), Some(l)) => out.push(((h << 4) | l) as u8),
                    // 不完整的 \x 序列按字面量保留
                    (h, l) => {
                        out.push(b'x');
                        if let Some(h) = h {
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(
                                char::from_digit(h, 16).unwrap().encode_utf8(&mut buf).as_bytes(),
                            );
                        }
                        if let Some(l) = l {
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(
                                char::from_digit(l, 16).unwrap().encode_utf8(&mut buf).as_bytes(),
                            );
                        }
                    }
                }
            }
            Some(other) => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
            }
            None => out.push(b'\\'),
        }
    }
    out
}

/// 控制字符可视化（llcom Byte2Readable 思路）：0x00-0x1F 映射到 U+2400 系符号，
/// 其余字符原样（UTF-8 安全）。
pub fn readable(bytes: &[u8]) -> String {
    const SYMBOLS: [char; 33] = [
        '␀', '␁', '␂', '␃', '␄', '␅', '␆', '␇', '␈', '␉', '␊', '␋', '␌', '␍', '␎', '␏', '␐',
        '␑', '␒', '␓', '␔', '␕', '␖', '␗', '␘', '␙', '␚', '␛', '␜', '␝', '␞', '␟', '␡',
    ];
    let mut s = String::with_capacity(bytes.len() + 8);
    let mut plain = Vec::new();
    for &b in bytes {
        match b {
            0x00..=0x1F | 0x7F => {
                if !plain.is_empty() {
                    s.push_str(&String::from_utf8_lossy(&plain));
                    plain.clear();
                }
                s.push(if b <= 0x1F { SYMBOLS[b as usize] } else { SYMBOLS[32] });
            }
            _ => plain.push(b),
        }
    }
    if !plain.is_empty() {
        s.push_str(&String::from_utf8_lossy(&plain));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_basic() {
        assert_eq!(parse_hex("AA 55").unwrap(), vec![0xAA, 0x55]);
        assert_eq!(parse_hex("aa55").unwrap(), vec![0xAA, 0x55]);
        assert_eq!(parse_hex("0xAA,0x1b,7f").unwrap(), vec![0xAA, 0x1B, 0x7F]);
        assert_eq!(parse_hex("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn parse_hex_errors() {
        assert!(parse_hex("AA5").is_err());
        assert!(parse_hex("ZZ").is_err());
    }

    #[test]
    fn roundtrip_hex() {
        let bytes = vec![0xDE, 0xAD, 0x01];
        assert_eq!(to_hex(&bytes, " "), "DE AD 01");
        assert_eq!(parse_hex(&to_hex(&bytes, "")).unwrap(), bytes);
    }

    #[test]
    fn unescape_crlf_and_hex() {
        assert_eq!(unescape("AT\\r\\n"), b"AT\r\n".to_vec());
        assert_eq!(unescape("\\xAA\\x55"), vec![0xAA, 0x55]);
        assert_eq!(unescape("a\\tb"), b"a\tb".to_vec());
        assert_eq!(unescape("a\\0b"), b"a\0b".to_vec());
    }

    #[test]
    fn unescape_chinese_passthrough() {
        assert_eq!(unescape("你好"), "你好".as_bytes().to_vec());
    }

    #[test]
    fn readable_control_chars() {
        assert_eq!(readable(b"A\r\nB"), "A␍␊B");
        assert_eq!(readable(&[0x7F]), "␡");
    }

    #[test]
    fn readable_utf8_safe() {
        let s = "串口测试\r\n";
        assert!(readable(s.as_bytes()).starts_with("串口测试"));
    }
}
