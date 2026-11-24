//! serial-pro 引擎库：串口工作线程、通道协议、配置持久化。
//!
//! 无 UI 依赖，可独立测试。P2 将在此扩展通用通道（TCP/UDP/MQTT/管道）
//! 与 Lua 双向钩子。

pub mod channel;
pub mod config;
pub mod error;
pub mod hexutil;
pub mod serial;

pub use channel::{ChannelCommand, ChannelEvent, SerialHub, SerialId};
pub use config::{Parity, PortConfig, QuickSendItem, Settings};
pub use error::{Error, Result};
