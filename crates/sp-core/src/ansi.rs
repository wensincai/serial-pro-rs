//! ANSI 转义序列解析：把接收字节渲染为着色文本段。
//!
//! 支持 SGR（0 复位 / 1 粗体 / 3 斜体 / 4 下划线 / 30-37 前景 / 38;5;n /
//! 38;2;r;g;b / 39 默认前景 / 90-97 亮色前景）；其余 CSI 序列（光标移动、
//! 清屏等）与 OSC 序列被剥离——调试控制台只关心颜色。

#[derive(Debug, Clone, PartialEq)]
pub struct AnsiSeg {
    pub text: String,
    /// None = 默认前景色
    pub fg: Option<[u8; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// 16 色 VGA 调色板（0-7 正常，8-15 亮色）。
const VGA16: [[u8; 3]; 16] = [
    [0, 0, 0],
    [205, 49, 49],
    [13, 188, 121],
    [229, 229, 16],
    [36, 114, 200],
    [188, 63, 188],
    [17, 168, 205],
    [229, 229, 229],
    [102, 102, 102],
    [255, 118, 118],
    [35, 209, 139],
    [255, 255, 100],
    [97, 175, 255],
    [255, 170, 255],
    [57, 219, 255],
    [255, 255, 255],
];

/// 256 色索引 -> RGB。
fn color256(idx: u8) -> [u8; 3] {
    match idx {
        0..=15 => VGA16[idx as usize],
        232..=255 => {
            let g = 8 + (idx - 232) as u16 * 10;
            [g as u8, g as u8, g as u8]
        }
        _ => {
            let i = idx - 16;
            let r = (i / 36).min(5) as u16;
            let g = ((i / 6) % 6).min(5) as u16;
            let b = (i % 6).min(5) as u16;
            let q = |v: u16| if v == 0 { 0 } else { 55 + v * 40 };
            [q(r) as u8, q(g) as u8, q(b) as u8]
        }
    }
}

struct Style {
    fg: Option<[u8; 3]>,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl Style {
    fn new() -> Self {
        Self {
            fg: None,
            bold: false,
            italic: false,
            underline: false,
        }
    }
    fn seg(&self, text: String) -> AnsiSeg {
        AnsiSeg {
            text,
            fg: self.fg,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
        }
    }
}

fn apply_sgr(style: &mut Style, params: &[u16]) {
    let mut i = 0;
    while i < params.len() {
        let p = params[i];
        match p {
            0 => *style = Style::new(),
            1 => style.bold = true,
            2 => style.bold = false, // dim 简化为不处理
            3 => style.italic = true,
            4 => style.underline = true,
            22 => style.bold = false,
            23 => style.italic = false,
            24 => style.underline = false,
            30..=37 => style.fg = Some(VGA16[(p - 30) as usize]),
            38 => {
                // 扩展前景：38;5;n 或 38;2;r;g;b
                if i + 1 < params.len() {
                    match params[i + 1] {
                        5 if i + 2 < params.len() => {
                            style.fg = Some(color256(params[i + 2].min(255) as u8));
                            i += 2;
                        }
                        2 if i + 4 < params.len() => {
                            style.fg = Some([
                                params[i + 2].min(255) as u8,
                                params[i + 3].min(255) as u8,
                                params[i + 4].min(255) as u8,
                            ]);
                            i += 4;
                        }
                        _ => {}
                    }
                }
            }
            39 => style.fg = None,
            90..=97 => style.fg = Some(VGA16[(p - 90 + 8) as usize]),
            _ => {} // 背景色等忽略
        }
        i += 1;
    }
}

/// 解析字节流为着色段。空段不产生；控制字符（\r\n\t 除外）被丢弃。
pub fn parse_ansi(data: &[u8]) -> Vec<AnsiSeg> {
    let mut segs: Vec<AnsiSeg> = Vec::new();
    let mut style = Style::new();
    let mut cur = String::new();
    let mut iter = data.iter().copied().peekable();

    macro_rules! flush {
        () => {
            if !cur.is_empty() {
                segs.push(style.seg(std::mem::take(&mut cur)));
            }
        };
    }

    while let Some(b) = iter.next() {
        match b {
            0x1B => {
                // 转义序列
                match iter.peek() {
                    Some(b'[') => {
                        iter.next();
                        // CSI: 参数字节 0x30-0x3F，中间字节 0x20-0x2F，终止 0x40-0x7E
                        let mut params_bytes = Vec::new();
                        let mut final_b = 0u8;
                        while let Some(&c) = iter.peek() {
                            if (0x30..=0x3F).contains(&c) {
                                params_bytes.push(c);
                                iter.next();
                            } else if (0x20..=0x2F).contains(&c) {
                                iter.next(); // 中间字节忽略
                            } else {
                                final_b = c;
                                iter.next();
                                break;
                            }
                        }
                        if final_b == b'm' {
                            flush!();
                            let params: Vec<u16> = String::from_utf8_lossy(&params_bytes)
                                .split([';', ':'])
                                .filter(|s| !s.is_empty())
                                .filter_map(|s| s.parse::<u16>().ok())
                                .collect();
                            // 无参数的 ESC[m == ESC[0m
                            if params.is_empty() {
                                style = Style::new();
                            } else {
                                apply_sgr(&mut style, &params);
                            }
                        }
                        // 其它 CSI（清屏/光标/滚动）直接剥离
                    }
                    Some(b']') => {
                        iter.next();
                        // OSC: 直到 BEL 或 ESC \
                        while let Some(c) = iter.next() {
                            if c == 0x07 {
                                break;
                            }
                            if c == 0x1B && iter.peek() == Some(&b'\\') {
                                iter.next();
                                break;
                            }
                        }
                    }
                    Some(_) => {
                        // 两字节转义（如 ESC c）剥离
                        iter.next();
                    }
                    None => {}
                }
            }
            b'\n' | b'\r' | b'\t' => {
                cur.push(b as char);
            }
            c if (0x20..0x7F).contains(&c) => {
                cur.push(c as char);
            }
            c if c >= 0x80 => {
                // UTF-8 多字节：攒够序列再解码（放宽处理：直接逐字节尝试）
                cur.push(c as char); // 占位，完整处理见下方说明
            }
            _ => {} // 其余控制字符丢弃
        }
    }
    flush!();

    // 修正 UTF-8：重新用 String::from_utf8_lossy 处理非 ASCII 部分
    // 简化方案：整包先按 UTF-8 lossy 转换，再在字符串层面重新解析一遍。
    // 为避免双遍逻辑，这里对逐字节 fallback（c as char 把高字节当 Latin-1）
    // 的场景做一次修正：若 data 是合法 UTF-8，用字符串版本重新解析。
    if String::from_utf8(data.to_vec()).is_ok() && data.iter().any(|&b| b >= 0x80) {
        let text = String::from_utf8_lossy(data);
        return parse_ansi_str(&text);
    }
    segs
}

/// 字符串版本（UTF-8 已合法时使用，正确处理中文等多字节字符）。
pub fn parse_ansi_str(text: &str) -> Vec<AnsiSeg> {
    let mut segs: Vec<AnsiSeg> = Vec::new();
    let mut style = Style::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();

    macro_rules! flush {
        () => {
            if !cur.is_empty() {
                segs.push(style.seg(std::mem::take(&mut cur)));
            }
        };
    }

    while let Some(c) = chars.next() {
        if c == '\u{1B}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    let mut params_bytes = String::new();
                    let mut final_b = ' ';
                    while let Some(&ch) = chars.peek() {
                        if ch.is_ascii_digit() || ch == ';' || ch == ':' || ch == '?' {
                            params_bytes.push(ch);
                            chars.next();
                        } else if ('\u{20}'..='\u{2F}').contains(&ch) {
                            chars.next();
                        } else {
                            final_b = ch;
                            chars.next();
                            break;
                        }
                    }
                    if final_b == 'm' {
                        flush!();
                        let params: Vec<u16> = params_bytes
                            .split([';', ':'])
                            .filter(|s| !s.is_empty())
                            .filter_map(|s| s.parse::<u16>().ok())
                            .collect();
                        if params.is_empty() {
                            style = Style::new();
                        } else {
                            apply_sgr(&mut style, &params);
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(ch) = chars.next() {
                        if ch == '\u{7}' {
                            break;
                        }
                        if ch == '\u{1B}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
        } else {
            cur.push(c);
        }
    }
    flush!();
    segs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs_text(segs: &[AnsiSeg]) -> String {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn plain_text_single_seg() {
        let segs = parse_ansi(b"hello world");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "hello world");
        assert_eq!(segs[0].fg, None);
    }

    #[test]
    fn basic_color() {
        let segs = parse_ansi(b"\x1b[31mred\x1b[0mwhite");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].fg, Some([205, 49, 49]));
        assert_eq!(segs[0].text, "red");
        assert_eq!(segs[1].fg, None);
        assert_eq!(segs[1].text, "white");
    }

    #[test]
    fn bold_and_combo_params() {
        let segs = parse_ansi(b"\x1b[1;32mgreen bold");
        assert!(segs[0].bold);
        assert_eq!(segs[0].fg, Some(VGA16[2]));
    }

    #[test]
    fn truecolor() {
        let segs = parse_ansi(b"\x1b[38;2;12;34;56mX");
        assert_eq!(segs[0].fg, Some([12, 34, 56]));
    }

    #[test]
    fn color_256_idx() {
        let segs = parse_ansi(b"\x1b[38;5;196mX");
        assert_eq!(segs[0].fg, Some(color256(196)));
    }

    #[test]
    fn csi_movement_stripped() {
        let segs = parse_ansi(b"\x1b[2J\x1b[H\x1b[3;5Hafter");
        assert_eq!(segs_text(&segs), "after");
    }

    #[test]
    fn osc_stripped() {
        let segs = parse_ansi(b"\x1b]0;title\x07data");
        assert_eq!(segs_text(&segs), "data");
        let segs = parse_ansi(b"\x1b]0;title\x1b\\data2");
        assert_eq!(segs_text(&segs), "data2");
    }

    #[test]
    fn utf8_chinese() {
        let segs = parse_ansi("温度：\u{1b}[31m高\u{1b}[0m".as_bytes());
        assert_eq!(segs_text(&segs), "温度：高");
        assert_eq!(segs[1].fg, Some([205, 49, 49]));
    }

    #[test]
    fn default_reset_when_empty_params() {
        let segs = parse_ansi(b"\x1b[31mx\x1b[my");
        assert_eq!(segs.len(), 2);
        assert!(segs[0].fg.is_some());
        assert_eq!(segs[1].fg, None);
    }

    #[test]
    fn newlines_kept() {
        let segs = parse_ansi(b"line1\r\nline2\ttab");
        assert_eq!(segs_text(&segs), "line1\r\nline2\ttab");
    }
}
