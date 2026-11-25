//! 通道协议：事件与命令。
//!
//! P0 只实现串口通道，但协议按通用通道设计——P2 增加 TCP/UDP/MQTT/管道
//! 通道时不改 UI 事件泵，只加新的 ChannelKind 实现。

use std::collections::HashMap;
use std::thread::JoinHandle;

use crossbeam_channel as mpmc;
use serde::{Deserialize, Serialize};

/// 串口通道 id（自增）
pub type SerialId = u32;

/// UI -> 通道命令
#[derive(Debug, Clone)]
pub enum ChannelCommand {
    Send(Vec<u8>),
    Close,
}

/// 通道 -> UI 事件
#[derive(Debug, Clone)]
pub enum ChannelEvent {
    Opened(SerialId),
    /// 收到数据（已按聚包窗口分帧）
    Rx(SerialId, Vec<u8>),
    /// 发送回显（方向箭头用）
    TxEcho(SerialId, Vec<u8>),
    Closed(SerialId, String),
    Error(SerialId, String),
}

/// 已激活的串口通道句柄
pub struct ActiveSerial {
    pub id: SerialId,
    pub cmd_tx: mpmc::Sender<ChannelCommand>,
    event_rx: mpmc::Receiver<ChannelEvent>,
    join: Option<JoinHandle<()>>,
}

impl ActiveSerial {
    pub fn try_recv(&self) -> Option<ChannelEvent> {
        self.event_rx.try_recv().ok()
    }

    /// 事件接收端克隆句柄（供测试/日志旁路使用）。
    pub fn event_handle(&self) -> mpmc::Receiver<ChannelEvent> {
        self.event_rx.clone()
    }

    pub fn send_bytes(&self, bytes: Vec<u8>) {
        let _ = self.cmd_tx.send(ChannelCommand::Send(bytes));
    }

    /// 关闭：发命令后由线程自行退出（UI 线程不直接 Drop 串口）。
    pub fn close(&mut self) {
        let _ = self.cmd_tx.send(ChannelCommand::Close);
    }

    /// 线程已结束后 join 回收。
    pub fn reap(&mut self) {
        if let Some(h) = self.join.take() {
            if !h.is_finished() {
                // 未结束则放回去，等下轮 reap
                self.join = Some(h);
            } else {
                let _ = h.join();
            }
        }
    }

    pub fn new(
        id: SerialId,
        cmd_tx: mpmc::Sender<ChannelCommand>,
        event_rx: mpmc::Receiver<ChannelEvent>,
        join: JoinHandle<()>,
    ) -> Self {
        Self {
            id,
            cmd_tx,
            event_rx,
            join: Some(join),
        }
    }
}

/// 串口通道管理器：支持同时打开多个端口。
#[derive(Default)]
pub struct SerialHub {
    next_id: SerialId,
    active: HashMap<SerialId, ActiveSerial>,
}

impl SerialHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self, cfg: &crate::config::PortConfig) -> Result<SerialId, crate::Error> {
        let id = self.next_id;
        self.next_id += 1;
        let active = crate::serial::spawn_serial(id, cfg.clone())?;
        self.active.insert(id, active);
        Ok(id)
    }

    /// 打开 TCP 客户端通道。
    pub fn open_tcp_client(&mut self, addr: &str) -> Result<SerialId, crate::Error> {
        let id = self.next_id;
        self.next_id += 1;
        let active = crate::net::spawn_tcp_client(id, addr)?;
        self.active.insert(id, active);
        Ok(id)
    }

    /// 打开 TCP 服务端通道（监听，支持多客户端）。
    pub fn open_tcp_server(&mut self, listen_addr: &str) -> Result<SerialId, crate::Error> {
        let id = self.next_id;
        self.next_id += 1;
        let active = crate::net::spawn_tcp_server(id, listen_addr)?;
        self.active.insert(id, active);
        Ok(id)
    }

    /// 打开 UDP 通道。
    pub fn open_udp(&mut self, bind_addr: &str, remote_addr: &str) -> Result<SerialId, crate::Error> {
        let id = self.next_id;
        self.next_id += 1;
        let active = crate::net::spawn_udp(id, bind_addr, remote_addr)?;
        self.active.insert(id, active);
        Ok(id)
    }

    /// 打开 MQTT 通道。
    pub fn open_mqtt(&mut self, params: &crate::mqtt::MqttParams) -> Result<SerialId, crate::Error> {
        let id = self.next_id;
        self.next_id += 1;
        let active = crate::mqtt::spawn_mqtt(id, params.clone())?;
        self.active.insert(id, active);
        Ok(id)
    }

    /// 打开 WinUSB 通道。
    pub fn open_winusb(&mut self, vid: u16, pid: u16) -> Result<SerialId, crate::Error> {
        let id = self.next_id;
        self.next_id += 1;
        let active = crate::usb::spawn_winusb(id, vid, pid)?;
        self.active.insert(id, active);
        Ok(id)
    }

    pub fn close(&mut self, id: SerialId) {
        if let Some(a) = self.active.get_mut(&id) {
            a.close();
        }
    }

    pub fn get(&self, id: SerialId) -> Option<&ActiveSerial> {
        self.active.get(&id)
    }

    pub fn ids(&self) -> Vec<SerialId> {
        let mut v: Vec<_> = self.active.keys().copied().collect();
        v.sort();
        v
    }

    pub fn is_open(&self, id: SerialId) -> bool {
        self.active.contains_key(&id)
    }

    /// 排水：把所有通道已结束的条目移除（收到 Closed 事件后调用方清理）。
    pub fn remove_if_closed(&mut self, id: SerialId) {
        if let Some(mut a) = self.active.remove(&id) {
            a.close();
            a.reap();
        }
    }

    /// 回收所有已结束线程的句柄。
    pub fn reap_all(&mut self) {
        for a in self.active.values_mut() {
            a.reap();
        }
    }
}

/// 供 UI 展示的通道元信息（非运行时数据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelKind {
    pub kind: String, // "serial" | "tcp-client" | ... P2 扩展
}
