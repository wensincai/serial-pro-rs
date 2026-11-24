//! 配置与持久化：每端口参数记忆、快捷发送条目。
//!
//! 写盘策略：显式动作触发（改参数/编辑快捷发送），不做属性 setter 全量写盘；
//! 写前滚动 .bak 备份。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Parity {
    #[default]
    None,
    Odd,
    Even,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortConfig {
    /// 端口名，如 "COM3"
    pub name: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: Parity,
    pub stop_bits: u8,
    /// 分包超时（毫秒）：>=0 为聚包窗口；<0 为流模式（读到即发）
    pub pack_timeout_ms: i64,
    pub hex_view: bool,
    pub hex_send: bool,
    /// 接收区最大保留条数（环形淘汰）
    pub max_records: usize,
}

impl Default for PortConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            baud: 115_200,
            data_bits: 8,
            parity: Parity::None,
            stop_bits: 1,
            pack_timeout_ms: 50,
            hex_view: false,
            hex_send: false,
            max_records: 2000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickSendItem {
    pub label: String,
    /// 原始内容；hex_send=true 时按 Hex 解析，否则按 UTF-8 文本（支持转义符）
    pub content: String,
    pub hex: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    /// 每端口参数记忆：端口名 -> 上次使用的配置
    #[serde(default)]
    pub ports: HashMap<String, PortConfig>,
    /// 快捷发送条目（单页；P1 扩展多页）
    #[serde(default)]
    pub quick_send: Vec<QuickSendItem>,
}

impl Settings {
    pub fn port_config(&self, port: &str) -> PortConfig {
        self.ports.get(port).cloned().unwrap_or_else(|| PortConfig {
            name: port.to_string(),
            ..Default::default()
        })
    }

    /// 取默认配置文件路径：exe 目录便携优先，否则 %APPDATA%。
    pub fn default_path() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            let portable = exe.parent().unwrap_or(Path::new(".")).join("settings.json");
            if portable.exists() {
                return portable;
            }
        }
        let base = std::env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        base.join("serial-pro-rs").join("settings.json")
    }

    /// 加载设置；失败时尝试 .bakup（llcom 同款恢复策略）。
    pub fn load(path: &Path) -> Settings {
        for p in [path.to_path_buf(), path.with_extension("json.bakup")] {
            if let Ok(text) = std::fs::read_to_string(&p) {
                if let Ok(s) = serde_json::from_str(&text) {
                    return s;
                }
            }
        }
        Settings::default()
    }

    /// 保存；先把旧文件滚动为 .bakup 再原子写临时文件。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if path.exists() {
            let bak = path.with_extension("json.bakup");
            let _ = std::fs::copy(path, &bak);
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let mut s = Settings::default();
        let cfg = PortConfig {
            name: "COM3".into(),
            baud: 9600,
            parity: Parity::Even,
            pack_timeout_ms: -1,
            ..Default::default()
        };
        s.ports.insert("COM3".into(), cfg);
        s.quick_send.push(QuickSendItem {
            label: "复位".into(),
            content: "AT+RST\\r\\n".into(),
            hex: false,
        });
        s.save(&path).unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(loaded.ports["COM3"].baud, 9600);
        assert_eq!(loaded.ports["COM3"].parity, Parity::Even);
        assert_eq!(loaded.ports["COM3"].pack_timeout_ms, -1);
        assert_eq!(loaded.quick_send.len(), 1);
        assert_eq!(loaded.quick_send[0].label, "复位");
    }

    #[test]
    fn load_corrupt_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "not json{{{").unwrap();
        let s = Settings::load(&path);
        assert!(s.ports.is_empty());
    }

    #[test]
    fn port_config_defaults_to_port_name() {
        let s = Settings::default();
        let cfg = s.port_config("COM7");
        assert_eq!(cfg.name, "COM7");
        assert_eq!(cfg.baud, 115_200);
    }
}
