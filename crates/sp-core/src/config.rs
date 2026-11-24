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
    /// ANSI 转义渲染（仅文本模式生效）
    #[serde(default)]
    pub ansi_view: bool,
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
            ansi_view: false,
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
    /// 快捷发送多页（P1：最多 10 页）
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quick_send: Vec<QuickSendItem>, // 旧字段：仅作迁移源，序列化时跳过
    #[serde(default)]
    pub quick_pages: Vec<Vec<QuickSendItem>>,
    /// 快捷发送页名（与 quick_pages 等长）
    #[serde(default)]
    pub quick_page_names: Vec<String>,
    /// 自动日志配置
    #[serde(default)]
    pub log: crate::logging::LogConfig,
    /// 帧结构发送配置
    #[serde(default)]
    pub frame: crate::frame::FrameSpec,
    /// 命令队列
    #[serde(default)]
    pub queue: crate::queue::CommandQueue,
    /// 定时发送间隔（毫秒），0 = 未启用
    #[serde(default)]
    pub timer_interval_ms: u64,
    /// Lua 双向钩子脚本
    #[serde(default)]
    pub lua_script: String,
    /// Lua 钩子是否启用
    #[serde(default)]
    pub lua_enabled: bool,
    /// 上次使用的通道类型："serial" | "tcp-client" | "tcp-server" | "udp"
    #[serde(default)]
    pub channel_kind: String,
    /// TCP 客户端目标地址
    #[serde(default)]
    pub tcp_client_addr: String,
    /// TCP 服务端监听地址
    #[serde(default)]
    pub tcp_listen_addr: String,
    /// UDP 本地绑定地址
    #[serde(default)]
    pub udp_bind_addr: String,
    /// UDP 远端目标地址
    #[serde(default)]
    pub udp_remote_addr: String,
}

impl Settings {
    /// 旧版单列表迁移到第 1 页；pages 为空时补一页空的。
    pub fn migrate(&mut self) {
        if self.quick_pages.is_empty() {
            let first = std::mem::take(&mut self.quick_send);
            self.quick_pages = vec![first];
        }
        self.quick_send.clear();
        while self.quick_page_names.len() < self.quick_pages.len() {
            let n = self.quick_page_names.len() + 1;
            self.quick_page_names.push(format!("第{n}页"));
        }
        if self.lua_script.is_empty() {
            self.lua_script = crate::hooks::DEFAULT_SCRIPT.to_string();
        }
        if self.channel_kind.is_empty() {
            self.channel_kind = "serial".into();
        }
        if self.tcp_listen_addr.is_empty() {
            self.tcp_listen_addr = "0.0.0.0:9000".into();
        }
        if self.udp_bind_addr.is_empty() {
            self.udp_bind_addr = "0.0.0.0:9001".into();
        }
        if self.udp_remote_addr.is_empty() {
            self.udp_remote_addr = "127.0.0.1:9000".into();
        }
        if self.tcp_client_addr.is_empty() {
            self.tcp_client_addr = "127.0.0.1:9000".into();
        }
    }

    pub fn pages(&self) -> &[Vec<QuickSendItem>] {
        &self.quick_pages
    }
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
                if let Ok(mut s) = serde_json::from_str::<Settings>(&text) {
                    s.migrate();
                    return s;
                }
            }
        }
        let mut s = Settings::default();
        s.migrate();
        s
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
        s.quick_pages.push(vec![QuickSendItem {
            label: "复位".into(),
            content: "AT+RST\\r\\n".into(),
            hex: false,
        }]);
        s.save(&path).unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(loaded.ports["COM3"].baud, 9600);
        assert_eq!(loaded.ports["COM3"].parity, Parity::Even);
        assert_eq!(loaded.ports["COM3"].pack_timeout_ms, -1);
        assert_eq!(loaded.quick_pages.len(), 1);
        assert_eq!(loaded.quick_pages[0][0].label, "复位");
    }

    #[test]
    fn migrate_legacy_quick_send() {
        // 旧版 JSON：只有 quick_send 单列表
        let legacy = r#"{
            "ports": {},
            "quick_send": [{"label": "旧条目", "content": "AA 55", "hex": true}]
        }"#;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, legacy).unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.quick_pages.len(), 1);
        assert_eq!(s.quick_pages[0][0].label, "旧条目");
        // 再保存后应使用 quick_pages 且跳过 quick_send 字段
        s.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("quick_pages"));
        assert!(!text.contains("quick_send"));
    }

    #[test]
    fn frame_and_queue_in_settings_roundtrip() {
        let mut s = Settings::default();
        s.frame.header = "AA55".into();
        s.frame.crc = crate::frame::CrcAlgo::Crc16Modbus;
        s.queue.items.push(crate::queue::QueueItem {
            label: "启动".into(),
            content: "01".into(),
            hex: true,
            delay_ms: 500,
        });
        s.timer_interval_ms = 1000;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        s.save(&path).unwrap();
        let back = Settings::load(&path);
        assert_eq!(back.frame.header, "AA55");
        assert_eq!(back.frame.crc, crate::frame::CrcAlgo::Crc16Modbus);
        assert_eq!(back.queue.items.len(), 1);
        assert_eq!(back.queue.items[0].delay_ms, 500);
        assert_eq!(back.timer_interval_ms, 1000);
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
