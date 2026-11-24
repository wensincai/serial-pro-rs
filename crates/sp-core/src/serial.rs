//! 串口工作线程：聚包读 + 发送 + 事件上报。
//!
//! 每端口一个线程，持有 serialport 句柄直至 Close 命令；UI 线程不触碰串口。
//! 读策略（借鉴 llcom/SuperCom 的延时聚包并消除 Sleep 轮询）：
//! - `pack_timeout_ms >= 0`：首包到达后进入聚包窗口，循环读直到
//!   `idle >= pack_timeout` 或单包 10KB 上限，把整个窗口作为一条记录上报；
//! - `pack_timeout_ms < 0`：流模式，读到即发（不做窗口聚合）。

use std::time::{Duration, Instant};

use crossbeam_channel as mpmc;
use serialport::SerialPort;

use crate::channel::{ActiveSerial, ChannelCommand, ChannelEvent, SerialId};
use crate::config::{Parity, PortConfig};
use crate::error::{Error, Result};

const READ_BUF: usize = 4096;
const MAX_PACKET: usize = 10 * 1024;

fn to_serial_parity(p: Parity) -> serialport::Parity {
    match p {
        Parity::None => serialport::Parity::None,
        Parity::Odd => serialport::Parity::Odd,
        Parity::Even => serialport::Parity::Even,
    }
}

/// 打开串口并拉起工作线程。
pub fn spawn_serial(id: SerialId, cfg: PortConfig) -> Result<ActiveSerial> {
    if cfg.name.is_empty() {
        return Err(Error::Serial("端口名为空".into()));
    }
    let port = serialport::new(&cfg.name, cfg.baud)
        .data_bits(match cfg.data_bits {
            7 => serialport::DataBits::Seven,
            _ => serialport::DataBits::Eight,
        })
        .parity(to_serial_parity(cfg.parity))
        .stop_bits(match cfg.stop_bits {
            2 => serialport::StopBits::Two,
            _ => serialport::StopBits::One,
        })
        .flow_control(serialport::FlowControl::None)
        // 读超时：轮询用，不能大于 IDLE_POLL 太多
        .timeout(Duration::from_millis(20))
        .open()
        .map_err(|e| Error::Serial(format!("打开 {} 失败: {e}", cfg.name)))?;

    let (cmd_tx, cmd_rx) = mpmc::unbounded::<ChannelCommand>();
    let (event_tx, event_rx) = mpmc::unbounded::<ChannelEvent>();

    let join = std::thread::Builder::new()
        .name(format!("sp-serial-{id}-{}", cfg.name))
        .spawn(move || worker_loop(id, port, cfg, cmd_rx, event_tx))
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;

    Ok(ActiveSerial::new(id, cmd_tx, event_rx, join))
}

fn worker_loop(
    id: SerialId,
    mut port: Box<dyn SerialPort>,
    cfg: PortConfig,
    cmd_rx: mpmc::Receiver<ChannelCommand>,
    event_tx: mpmc::Sender<ChannelEvent>,
) {
    let _ = event_tx.send(ChannelEvent::Opened(id));
    let mut buf = [0u8; READ_BUF];
    let stream_mode = cfg.pack_timeout_ms < 0;
    let window = if stream_mode {
        Duration::from_millis(0)
    } else {
        Duration::from_millis(cfg.pack_timeout_ms.unsigned_abs())
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        loop {
            // 1. 先处理命令（非阻塞）
            match cmd_rx.try_recv() {
                Ok(ChannelCommand::Close) | Err(mpmc::TryRecvError::Disconnected) => break,
                Ok(ChannelCommand::Send(bytes)) => {
                    match port.write_all(&bytes) {
                        Ok(()) => {
                            let _ = port.flush();
                            let _ = event_tx.send(ChannelEvent::TxEcho(id, bytes));
                        }
                        Err(e) => {
                            let _ = event_tx.send(ChannelEvent::Error(id, format!("写失败: {e}")));
                        }
                    }
                }
                Err(mpmc::TryRecvError::Empty) => {}
            }

            // 2. 读数据
            match port.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    let mut packet = buf[..n].to_vec();
                    if !stream_mode {
                        // 聚包窗口：继续读直到 idle 超过 pack_timeout 或达到上限
                        let start = Instant::now();
                        while packet.len() < MAX_PACKET {
                            if start.elapsed() >= window {
                                break;
                            }
                            match port.read(&mut buf) {
                                Ok(m) if m > 0 => packet.extend_from_slice(&buf[..m]),
                                _ => {} // 超时/暂时无数据，继续等窗口
                            }
                        }
                    }
                    let _ = event_tx.send(ChannelEvent::Rx(id, packet));
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    // 读超时即空转一轮，顺带保证命令通道每 ~20ms 被检查一次
                }
                Err(e) => {
                    // 设备拔出等致命错误：上报后正常走关闭流程
                    let _ = event_tx.send(ChannelEvent::Error(id, format!("读失败: {e}")));
                    break;
                }
            }
        }
    }));

    if result.is_err() {
        let _ = event_tx.send(ChannelEvent::Error(id, "串口线程 panic".into()));
    }
    let _ = event_tx.send(ChannelEvent::Closed(id, "closed".into()));
    drop(port);
}
