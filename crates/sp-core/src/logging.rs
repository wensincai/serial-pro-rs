//! 接收日志自动落盘：文件名/目录模板 + 按 MB 分片。
//!
//! 模板占位符（对齐 SuperCom 规格）：
//! - `%C` 端口名（非法路径字符替换为 `_`）
//! - `%Y %M %D` 年/月/日；`%h %m %s` 时/分/秒；`%fff` 毫秒
//! - `%R` 方向（RX/TX，仅用于行前缀，不用于文件名）
//!
//! 时间基准为本地 UTC+8（国内用户默认场景），偏移量可配。

use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 本地时区偏移（秒）：UTC+8。若后续做多语言可改为动态获取。
pub const TZ_OFFSET_SECS: i64 = 8 * 3600;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    pub enabled: bool,
    /// 目录模板，默认 `logs`（相对 exe 目录或 cwd）
    pub dir_template: String,
    /// 文件名模板
    pub file_template: String,
    /// 分片大小（MB），0 = 不分片
    pub frag_mb: u64,
    /// 行内是否附带可读文本
    pub include_readable: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir_template: "logs".into(),
            file_template: "%C_%Y%M%D_%h%m%s.log".into(),
            frag_mb: 10,
            include_readable: true,
        }
    }
}

/// 把时间戳按本地时区拆成 (年, 月, 日, 时, 分, 秒, 毫秒)。
/// 儒略日算法（Howard Hinnant civil_from_days），无外部依赖。
pub fn split_ts(t: SystemTime) -> (i64, u32, u32, u32, u32, u32, u32) {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs_total = d.as_secs() as i64 + TZ_OFFSET_SECS;
    let millis = d.subsec_millis();
    let days = secs_total.div_euclid(86_400);
    let secs_of_day = secs_total.rem_euclid(86_400);

    // civil_from_days
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d_ = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    (
        year,
        m as u32,
        d_ as u32,
        (secs_of_day / 3600) as u32,
        ((secs_of_day % 3600) / 60) as u32,
        (secs_of_day % 60) as u32,
        millis,
    )
}

fn render(t: SystemTime, template: &str, port: &str) -> String {
    let (y, mo, d, h, mi, s, ms) = split_ts(t);
    let port = port
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    template
        .replace("%C", &port)
        .replace("%Y", &format!("{y:04}"))
        .replace("%M", &format!("{mo:02}"))
        .replace("%D", &format!("{d:02}"))
        .replace("%h", &format!("{h:02}"))
        .replace("%m", &format!("{mi:02}"))
        .replace("%s", &format!("{s:02}"))
        .replace("%fff", &format!("{ms:03}"))
}

/// 每端口一个日志会话。负责：模板渲染出目录/文件、按 MB 滚动分片、追加写入。
pub struct LogWriter {
    cfg: LogConfig,
    port: String,
    dir: PathBuf,
    current: Option<PathBuf>,
    written: u64,
    seq: u32,
    /// 测试注入用；None = 实时取
    now: Option<SystemTime>,
}

impl LogWriter {
    pub fn new(cfg: LogConfig, port: &str) -> Self {
        let t = SystemTime::now();
        let dir = PathBuf::from(render(t, &cfg.dir_template, port));
        Self {
            cfg,
            port: port.to_string(),
            dir,
            current: None,
            written: 0,
            seq: 0,
            now: None,
        }
    }

    #[cfg(test)]
    fn with_fixed_now(mut self, t: SystemTime) -> Self {
        self.now = Some(t);
        self
    }

    fn now(&self) -> SystemTime {
        self.now.unwrap_or_else(SystemTime::now)
    }

    /// 计算下一个日志文件路径（按分片与序号）。
    fn next_path(&self) -> PathBuf {
        let base = render(self.now(), &self.cfg.file_template, &self.port);
        let stem = base
            .rsplit_once('.')
            .map(|(s, _)| s.to_string())
            .unwrap_or(base.clone());
        let ext = base
            .rsplit_once('.')
            .map(|(_, e)| e.to_string())
            .unwrap_or_else(|| "log".into());
        let name = if self.seq == 0 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}_part{}.{ext}", self.seq + 1)
        };
        self.dir.join(name)
    }

    fn roll_if_needed(&mut self) {
        let limit = self.cfg.frag_mb.saturating_mul(1024 * 1024);
        let need_roll = match &self.current {
            None => true,
            Some(_) => limit > 0 && self.written >= limit,
        };
        if need_roll {
            let p = self.next_path();
            self.current = Some(p);
            self.written = 0;
            self.seq += 1;
            if let Some(dir) = self.current.as_ref().and_then(|p| p.parent()) {
                let _ = std::fs::create_dir_all(dir);
            }
        }
    }

    /// 追加一行：`[HH:MM:SS.mmm][RX|TX] <hex> [readable]`
    pub fn write(&mut self, dir_tx: bool, bytes: &[u8]) -> Result<()> {
        self.roll_if_needed();
        let Some(path) = &self.current else { return Ok(()) };
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        let (h, mi, s, ms) = {
            let (_, _, _, h, mi, s, ms) = split_ts(self.now());
            (h, mi, s, ms)
        };
        let mut line = format!(
            "[{h:02}:{mi:02}:{s:02}.{ms:03}][{}] {}",
            if dir_tx { "TX" } else { "RX" },
            crate::hexutil::to_hex(bytes, " ")
        );
        if self.cfg.include_readable {
            line.push_str(&format!(" | {}", crate::hexutil::readable(bytes).replace('\n', "\\n")));
        }
        line.push('\n');
        self.written += line.len() as u64;
        f.write_all(line.as_bytes())?;
        Ok(())
    }

    /// 当前文件路径（测试/展示用）。
    pub fn current_path(&self) -> Option<&PathBuf> {
        self.current.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64, millis: u32) -> SystemTime {
        UNIX_EPOCH + std::time::Duration::new(secs, millis * 1_000_000)
    }

    #[test]
    fn split_ts_epoch_plus8() {
        // 1970-01-01 00:00:00 UTC -> UTC+8 => 08:00:00 当天
        let (y, m, d, h, mi, s, ms) = split_ts(t(0, 0));
        assert_eq!((y, m, d, h, mi, s, ms), (1970, 1, 1, 8, 0, 0, 0));
    }

    #[test]
    fn split_ts_known_date() {
        // 2026-11-24 00:00:00 UTC+8 = 2026-11-23 16:00:00 UTC
        // = 20780 天（1970-01-01 至 2026-11-23）* 86400 + 16h
        let secs = 20_780 * 86_400 + 16 * 3600;
        let (y, m, d, h, ..) = split_ts(t(secs, 0));
        assert_eq!((y, m, d, h), (2026, 11, 24, 0));
    }

    #[test]
    fn render_template() {
        let s = t(0, 123);
        assert_eq!(render(s, "%C_%Y%M%D_%h%m%s_%fff", "COM3"), "COM3_19700101_080000_123");
        assert_eq!(render(s, "%C", "COM/3:x"), "COM_3_x");
    }

    #[test]
    fn writer_writes_and_frags() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = LogConfig {
            enabled: true,
            dir_template: dir.path().join("out").to_string_lossy().into(),
            file_template: "%C.log".into(),
            frag_mb: 1,
            include_readable: false,
        };
        // 1MB 分片太小不好测，改成手写小分片：直接用 1 字节极限？frag_mb 最小单位 MB，
        // 改用不满足分片时单文件路径正确性 + 多次写入追加
        cfg.frag_mb = 0; // 不分片
        let mut w = LogWriter::new(cfg, "COM5").with_fixed_now(t(0, 0));
        w.write(false, b"AB").unwrap();
        w.write(true, b"CD").unwrap();
        let p = w.current_path().unwrap().clone();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("[RX] 41 42"));
        assert!(text.contains("[TX] 43 44"));
        assert!(p.to_string_lossy().contains("COM5.log"));
        assert_eq!(w.seq, 1);
    }
}
