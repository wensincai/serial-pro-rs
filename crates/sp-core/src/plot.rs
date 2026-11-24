//! 数值提取与滚动序列：为示波器/曲线绘图供数。
//!
//! 文本模式：从接收字节里提取所有可解析的十进制数（支持小数/负数/科学计数），
//! 按出现顺序作为该包的第 1..N 个数值列；
//! Hex 模式：每字节作为一列（0..255）。

use std::collections::VecDeque;

#[derive(Debug, Clone, Default)]
pub struct PlotSeries {
    /// (x = 采样序号, y = 数值)
    pub points: VecDeque<(f64, f64)>,
}

impl PlotSeries {
    pub const MAX_POINTS: usize = 4000;

    pub fn push(&mut self, x: f64, y: f64) {
        if self.points.len() >= Self::MAX_POINTS {
            self.points.pop_front();
        }
        self.points.push_back((x, y));
    }
}

/// 从一包数据提取数值序列。
pub fn extract_numbers(data: &[u8], hex_view: bool) -> Vec<f64> {
    if hex_view {
        return data.iter().map(|&b| b as f64).collect();
    }
    let text = String::from_utf8_lossy(data);
    extract_numbers_from_text(&text)
}

pub fn extract_numbers_from_text(text: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let n = chars.len();
    while i < n {
        let c = chars[i];
        // 数字起点：数字 / 符号后跟数字或 '.'
        let starts_num = c.is_ascii_digit()
            || ((c == '-' || c == '+' || c == '.')
                && chars
                    .get(i + 1)
                    .map(|nc| nc.is_ascii_digit() || *nc == '.')
                    .unwrap_or(false)
                && (c != '.' || chars.get(i + 1).map(|nc| nc.is_ascii_digit()).unwrap_or(false)));
        if !starts_num {
            i += 1;
            continue;
        }
        let start = i;
        // 吃掉符号
        if c == '-' || c == '+' {
            i += 1;
        }
        let mut seen_digit = false;
        let mut seen_dot = false;
        while i < n {
            let ch = chars[i];
            if ch.is_ascii_digit() {
                seen_digit = true;
                i += 1;
            } else if ch == '.' && !seen_dot {
                seen_dot = true;
                i += 1;
            } else {
                break;
            }
        }
        // 可选科学计数
        if i < n && (chars[i] == 'e' || chars[i] == 'E') {
            let mut j = i + 1;
            if j < n && (chars[j] == '-' || chars[j] == '+') {
                j += 1;
            }
            if j < n && chars[j].is_ascii_digit() {
                i = j;
                while i < n && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
        }
        if seen_digit {
            let token: String = chars[start..i].iter().collect();
            if let Ok(v) = token.parse::<f64>() {
                out.push(v);
            }
        }
    }
    out
}

/// 滚动曲线集：最多 MAX_SERIES 条曲线。
pub struct PlotData {
    pub series: Vec<PlotSeries>,
    next_x: u64,
}

pub const MAX_SERIES: usize = 3;

impl Default for PlotData {
    fn default() -> Self {
        Self {
            series: (0..MAX_SERIES).map(|_| PlotSeries::default()).collect(),
            next_x: 0,
        }
    }
}

impl PlotData {
    /// 喂入一包：把第 k 列写进第 k 条曲线。
    pub fn feed(&mut self, data: &[u8], hex_view: bool) {
        let nums = extract_numbers(data, hex_view);
        let x = self.next_x as f64;
        self.next_x = self.next_x.wrapping_add(1);
        for (k, v) in nums.iter().take(MAX_SERIES).enumerate() {
            self.series[k].push(x, *v);
        }
    }

    pub fn clear(&mut self) {
        for s in &mut self.series {
            s.points.clear();
        }
        self.next_x = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_simple() {
        let v = extract_numbers_from_text("t=25.3, h=60, n=-3");
        assert_eq!(v, vec![25.3, 60.0, -3.0]);
    }

    #[test]
    fn extract_csv_and_scientific() {
        let v = extract_numbers_from_text("1,2.5,3e2,-4E-1");
        assert_eq!(v, vec![1.0, 2.5, 300.0, -0.4]);
    }

    #[test]
    fn ignores_partial_tokens() {
        // COM3 里的 3 会被提取（宽松行为，sscom 同款）
        let v = extract_numbers_from_text("COM3 temp:");
        assert_eq!(v, vec![3.0]);
        // 版本号 1.2.3：宽松提取 1.2 和 0.3（.3 视为小数 0.3）
        let v = extract_numbers_from_text("v1.2.3");
        assert_eq!(v, vec![1.2, 0.3]);
    }

    #[test]
    fn hex_mode_per_byte() {
        let v = extract_numbers(&[0x00, 0xFF, 0x10], true);
        assert_eq!(v, vec![0.0, 255.0, 16.0]);
    }

    #[test]
    fn feed_distributes_columns() {
        let mut pd = PlotData::default();
        pd.feed(b"10 20 30", false);
        pd.feed(b"11 21 31", false);
        assert_eq!(pd.series[0].points.len(), 2);
        assert_eq!(pd.series[1].points[1].1, 21.0);
        assert_eq!(pd.series[2].points[0].1, 30.0);
    }

    #[test]
    fn series_ring_buffer() {
        let mut s = PlotSeries::default();
        for i in 0..(PlotSeries::MAX_POINTS + 10) {
            s.push(i as f64, i as f64);
        }
        assert_eq!(s.points.len(), PlotSeries::MAX_POINTS);
        assert_eq!(s.points.front().unwrap().0, 10.0);
    }
}
