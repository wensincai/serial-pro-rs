//! serial-pro 引擎库：串口工作线程、通道协议、配置持久化。
//!
//! 无 UI 依赖，可独立测试。P2 将在此扩展通用通道（TCP/UDP/MQTT/管道）
//! 与 Lua 双向钩子。

pub mod ansi;
pub mod channel;
pub mod config;
pub mod error;
pub mod frame;
pub mod hexutil;
pub mod hooks;
pub mod logging;
pub mod net;
pub mod plot;
pub mod queue;
pub mod serial;

pub use ansi::AnsiSeg;
pub use channel::{ChannelCommand, ChannelEvent, SerialHub, SerialId};
pub use config::{Parity, PortConfig, QuickSendItem, Settings};
pub use error::{Error, Result};
pub use frame::{CrcAlgo, CrcRange, FrameSpec};
pub use hooks::LuaHooks;
pub use plot::PlotData;
pub use queue::{CommandQueue, QueueCursor, QueueItem};
