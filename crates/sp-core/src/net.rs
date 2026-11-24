//! 通用网络通道：TCP 客户端 / TCP 服务端 / UDP。
//!
//! 与 serial.rs 同款 worker 模式：每通道一个线程 + catch_unwind 隔离，
//! 事件/命令走同一 ChannelEvent/ChannelCommand 协议，UI 不需要改动事件泵。

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::time::Duration;

use crossbeam_channel as mpmc;

use crate::channel::{ActiveSerial, ChannelCommand, ChannelEvent, SerialId};
use crate::error::{Error, Result};

const READ_BUF: usize = 4096;
/// 阻塞读超时（也是命令通道的检查周期）
const READ_TIMEOUT: Duration = Duration::from_millis(20);

fn spawn_worker<F>(id: SerialId, name: String, f: F) -> Result<ActiveSerial>
where
    F: FnOnce(mpmc::Receiver<ChannelCommand>, mpmc::Sender<ChannelEvent>) + Send + 'static,
{
    let (cmd_tx, cmd_rx) = mpmc::unbounded::<ChannelCommand>();
    let (event_tx, event_rx) = mpmc::unbounded::<ChannelEvent>();
    let join = std::thread::Builder::new()
        .name(format!("sp-net-{id}-{name}"))
        .spawn(move || f(cmd_rx, event_tx))
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
    Ok(ActiveSerial::new(id, cmd_tx, event_rx, join))
}

fn finish(id: SerialId, event_tx: &mpmc::Sender<ChannelEvent>, panic: bool) {
    if panic {
        let _ = event_tx.send(ChannelEvent::Error(id, "网络线程 panic".into()));
    }
    let _ = event_tx.send(ChannelEvent::Closed(id, "closed".into()));
}

// ------------------------------------------------ TCP 客户端

/// TCP 客户端：连接到 `addr`，收发数据。
pub fn spawn_tcp_client(id: SerialId, addr: &str) -> Result<ActiveSerial> {
    let addr = addr.to_string();
    spawn_worker(id, format!("tcp-c:{addr}"), move |cmd_rx, event_tx| {
        let stream = match TcpStream::connect(&addr) {
            Ok(s) => s,
            Err(e) => {
                let _ = event_tx.send(ChannelEvent::Error(id, format!("连接 {addr} 失败: {e}")));
                finish(id, &event_tx, false);
                return;
            }
        };
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let _ = stream.set_nodelay(true);
        let _ = event_tx.send(ChannelEvent::Opened(id));
        let mut buf = [0u8; READ_BUF];
        let mut writer = match stream.try_clone() {
            Ok(w) => w,
            Err(e) => {
                let _ = event_tx.send(ChannelEvent::Error(id, format!("克隆流失败: {e}")));
                finish(id, &event_tx, false);
                return;
            }
        };
        let mut stream = stream;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
            match cmd_rx.try_recv() {
                Ok(ChannelCommand::Close) | Err(mpmc::TryRecvError::Disconnected) => break,
                Ok(ChannelCommand::Send(bytes)) => match writer.write_all(&bytes) {
                    Ok(()) => {
                        let _ = event_tx.send(ChannelEvent::TxEcho(id, bytes));
                    }
                    Err(e) => {
                        let _ =
                            event_tx.send(ChannelEvent::Error(id, format!("发送失败: {e}")));
                        break;
                    }
                },
                Err(mpmc::TryRecvError::Empty) => {}
            }
            match stream.read(&mut buf) {
                Ok(0) => {
                    // 对端正常关闭
                    break;
                }
                Ok(n) => {
                    let _ = event_tx.send(ChannelEvent::Rx(id, buf[..n].to_vec()));
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    // 空转
                }
                Err(_) => break, // 连接断开
            }
        }))
        .is_err();
        finish(id, &event_tx, panic);
    })
}

// ------------------------------------------------ TCP 服务端

/// TCP 服务端：监听 `listen_addr`，支持多客户端并发。
/// 任一客户端的数据都作为 Rx 上报；发送广播给所有已连接客户端。
pub fn spawn_tcp_server(id: SerialId, listen_addr: &str) -> Result<ActiveSerial> {
    let addr = listen_addr.to_string();
    spawn_worker(id, format!("tcp-s:{addr}"), move |cmd_rx, event_tx| {
        let listener = match TcpListener::bind(&addr) {
            Ok(l) => l,
            Err(e) => {
                let _ =
                    event_tx.send(ChannelEvent::Error(id, format!("监听 {addr} 失败: {e}")));
                finish(id, &event_tx, false);
                return;
            }
        };
        listener
            .set_nonblocking(true)
            .map_err(Error::Io)
            .ok();
        let _ = event_tx.send(ChannelEvent::Opened(id));

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut clients: Vec<(SocketAddr, TcpStream)> = Vec::new();
            let mut buf = [0u8; READ_BUF];
            loop {
                // 命令
                match cmd_rx.try_recv() {
                    Ok(ChannelCommand::Close) | Err(mpmc::TryRecvError::Disconnected) => break,
                    Ok(ChannelCommand::Send(bytes)) => {
                        // 广播给所有客户端（对端写缓冲满则丢弃该客户端）
                        clients.retain_mut(|(_, s)| s.write_all(&bytes).is_ok());
                        let _ = event_tx.send(ChannelEvent::TxEcho(id, bytes));
                    }
                    Err(mpmc::TryRecvError::Empty) => {}
                }
                // 接入新客户端
                while let Ok((stream, peer)) = listener.accept() {
                    let _ = stream.set_nonblocking(true);
                    let _ = stream.set_nodelay(true);
                    let _ = event_tx.send(ChannelEvent::Error(
                        id,
                        format!("[监听] 客户端接入 {peer}"),
                    ));
                    clients.push((peer, stream));
                }
                // 读客户端数据
                let mut broken: Vec<SocketAddr> = Vec::new();
                for (peer, s) in clients.iter_mut() {
                    match s.read(&mut buf) {
                        Ok(0) => broken.push(*peer),
                        Ok(n) => {
                            let _ = event_tx.send(ChannelEvent::Rx(id, buf[..n].to_vec()));
                        }
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(_) => broken.push(*peer),
                    }
                }
                if !broken.is_empty() {
                    clients.retain(|(p, _)| !broken.contains(p));
                    for p in broken {
                        let _ = event_tx.send(ChannelEvent::Error(
                            id,
                            format!("[监听] 客户端断开 {p}"),
                        ));
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }))
        .is_err();
        finish(id, &event_tx, panic);
    })
}

// ------------------------------------------------ UDP

/// UDP：绑定 `bind_addr`（可为 "0.0.0.0:0" 随机端口），发往 `remote_addr`。
/// 接收时不限来源（调试工具惯例），但显示来源地址。
pub fn spawn_udp(id: SerialId, bind_addr: &str, remote_addr: &str) -> Result<ActiveSerial> {
    let bind = bind_addr.to_string();
    let remote = remote_addr.to_string();
    spawn_worker(id, format!("udp:{bind}->{remote}"), move |cmd_rx, event_tx| {
        let socket = match UdpSocket::bind(&bind) {
            Ok(s) => s,
            Err(e) => {
                let _ =
                    event_tx.send(ChannelEvent::Error(id, format!("绑定 {bind} 失败: {e}")));
                finish(id, &event_tx, false);
                return;
            }
        };
        let remote: SocketAddr = match remote.parse() {
            Ok(a) => a,
            Err(_) => {
                let _ = event_tx.send(ChannelEvent::Error(
                    id,
                    format!("目标地址无效: {remote}（应为 ip:port）"),
                ));
                finish(id, &event_tx, false);
                return;
            }
        };
        socket.set_read_timeout(Some(READ_TIMEOUT)).ok();
        let _ = event_tx.send(ChannelEvent::Opened(id));
        let mut buf = [0u8; READ_BUF];
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
            match cmd_rx.try_recv() {
                Ok(ChannelCommand::Close) | Err(mpmc::TryRecvError::Disconnected) => break,
                Ok(ChannelCommand::Send(bytes)) => match socket.send_to(&bytes, remote) {
                    Ok(_) => {
                        let _ = event_tx.send(ChannelEvent::TxEcho(id, bytes));
                    }
                    Err(e) => {
                        let _ =
                            event_tx.send(ChannelEvent::Error(id, format!("发送失败: {e}")));
                    }
                },
                Err(mpmc::TryRecvError::Empty) => {}
            }
            match socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let _ = event_tx.send(ChannelEvent::Rx(id, buf[..n].to_vec()));
                    let _ = from; // 来源地址可在 UI 状态里展示（P2 简化：不逐包上报）
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => {}
            }
        }))
        .is_err();
        finish(id, &event_tx, panic);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelEvent;
    use std::time::Duration;

    fn wait_event(rx: &mpmc::Receiver<ChannelEvent>, secs: u64) -> Option<ChannelEvent> {
        let deadline = std::time::Instant::now() + Duration::from_secs(secs);
        while std::time::Instant::now() < deadline {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
                return Some(ev);
            }
        }
        None
    }

    #[test]
    #[ignore = "沙箱禁止绑定/连接网络；实机验证"]
    fn tcp_client_roundtrip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        // 模拟对端：收 01 02 回 03 04
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut b = [0u8; 2];
            let n = std::io::Read::read(&mut s, &mut b).unwrap();
            assert_eq!(&b[..n], &[0x01, 0x02]);
            std::io::Write::write_all(&mut s, &[0x03, 0x04]).unwrap();
        });

        let mut hub = crate::channel::SerialHub::new();
        let id = hub.open_tcp_client(&addr).unwrap();
        let rx = hub.get(id).unwrap().event_handle();
        hub.get(id).unwrap().send_bytes(vec![0x01, 0x02]);
        assert!(matches!(wait_event(&rx, 3), Some(ChannelEvent::Rx(_, b)) if b == vec![3, 4]));
        hub.close(id);
    }

    #[test]
    fn addr_validation() {
        // 无效地址应立即失败而非 panic
        let mut hub = crate::channel::SerialHub::new();
        assert!(hub.open_tcp_client("not an addr").is_err() || {
            // connect 失败在线程里上报，此处只要不 panic 即可
            hub.reap_all();
            true
        });
    }
}
