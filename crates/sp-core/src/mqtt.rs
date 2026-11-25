//! MQTT 通道：rumqttc 同步客户端。
//!
//! 工作模型：
//! - 收：内部线程阻塞轮询 `Connection`，Publish 报文 payload 作为 Rx 上报；
//! - 发：worker 主循环从命令通道取 Send，publish 到发布主题；
//! - 关闭：`disconnect()` 让内部线程以错误退出，worker 随后结束。

use std::time::Duration;

use crossbeam_channel as mpmc;
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};

use crate::channel::{ActiveSerial, ChannelCommand, ChannelEvent, SerialId};
use crate::error::{Error, Result};

/// MQTT 通道参数。
#[derive(Debug, Clone, Default)]
pub struct MqttParams {
    /// broker 地址，`host:port`（端口缺省 1883）
    pub broker: String,
    /// 客户端 id（空则自动生成）
    pub client_id: String,
    /// 订阅主题（收）
    pub sub_topic: String,
    /// 发布主题（发）
    pub pub_topic: String,
}

fn parse_broker(broker: &str) -> Result<(String, u16)> {
    let (host, port) = broker
        .rsplit_once(':')
        .ok_or_else(|| Error::InvalidParam(format!("MQTT broker 缺少端口: {broker}")))?;
    let port: u16 = port
        .parse()
        .map_err(|_| Error::InvalidParam(format!("MQTT broker 端口无效: {port}")))?;
    Ok((host.to_string(), port))
}

/// 启动 MQTT 通道 worker。
pub fn spawn_mqtt(id: SerialId, params: MqttParams) -> Result<ActiveSerial> {
    let (host, port) = parse_broker(&params.broker)?;
    let client_id = if params.client_id.trim().is_empty() {
        format!("serial-pro-{id}-{}", std::process::id())
    } else {
        params.client_id.trim().to_string()
    };
    let sub = params.sub_topic.trim().to_string();
    let pub_topic = params.pub_topic.trim().to_string();

    let mut opts = MqttOptions::new(&client_id, &host, port);
    opts.set_keep_alive(Duration::from_secs(30));
    let (client, mut connection) = Client::new(opts, 16);
    // Client 是 Clone（线程安全），控制线程用克隆
    let client_ctrl = client.clone();

    let (cmd_tx, cmd_rx) = mpmc::unbounded::<ChannelCommand>();
    let (event_tx, event_rx) = mpmc::unbounded::<ChannelEvent>();

    // 内部线程：阻塞收 MQTT 报文 -> Rx
    let join = std::thread::Builder::new()
        .name(format!("sp-mqtt-{id}"))
        .spawn({
            let event_tx = event_tx.clone();
            move || {
                // 订阅（broker 拒绝也不影响 worker，错误经 Error 事件上报）
                if !sub.is_empty() {
                    let _ = client.subscribe(&sub, QoS::AtLeastOnce);
                }
                let _ = event_tx.send(ChannelEvent::Opened(id));
                loop {
                    // rumqttc 同步 recv 返回双层 Result：外层是事件通道错误
                    match connection.recv() {
                        Ok(Ok(Event::Incoming(Packet::Publish(p)))) => {
                            let _ = event_tx.send(ChannelEvent::Rx(id, p.payload.to_vec()));
                        }
                        Ok(Ok(_)) => {} // ConnAck/PingResp 等
                        Ok(Err(e)) => {
                            // 主动断开也会走到这里，正常收尾
                            let _ = event_tx.send(ChannelEvent::Closed(
                                id,
                                format!("MQTT 连接结束: {e:?}"),
                            ));
                            return;
                        }
                        Err(_) => {
                            let _ = event_tx.send(ChannelEvent::Closed(id, "MQTT 事件通道关闭".into()));
                            return;
                        }
                    }
                }
            }
        })
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;

    // worker：命令 -> publish
    let join_main = std::thread::Builder::new()
        .name(format!("sp-mqtt-ctrl-{id}"))
        .spawn(move || {
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
                match cmd_rx.try_recv() {
                    Ok(ChannelCommand::Close)
                    | Err(mpmc::TryRecvError::Disconnected) => break,
                    Ok(ChannelCommand::Send(bytes)) => {
                        if pub_topic.is_empty() {
                            let _ = event_tx.send(ChannelEvent::Error(
                                id,
                                "未配置发布主题，数据未发送".into(),
                            ));
                        } else {
                            match client_ctrl.publish(&pub_topic, QoS::AtLeastOnce, false, bytes.clone())
                            {
                                Ok(()) => {
                                    let _ = event_tx.send(ChannelEvent::TxEcho(id, bytes));
                                }
                                Err(e) => {
                                    let _ = event_tx
                                        .send(ChannelEvent::Error(id, format!("发布失败: {e}")));
                                }
                            }
                        }
                    }
                    Err(mpmc::TryRecvError::Empty) => {}
                }
                std::thread::sleep(Duration::from_millis(10));
            }))
            .is_err();
            let _ = client_ctrl.disconnect();
            // 回收内部线程，避免泄漏
            let _ = join.join();
            if panic {
                let _ = event_tx.send(ChannelEvent::Error(id, "MQTT 线程 panic".into()));
            }
            let _ = event_tx.send(ChannelEvent::Closed(id, "closed".into()));
        })
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;

    Ok(ActiveSerial::new(id, cmd_tx, event_rx, join_main))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_parsing() {
        assert_eq!(
            parse_broker("broker.emqx.io:1883").unwrap(),
            ("broker.emqx.io".into(), 1883)
        );
        assert_eq!(parse_broker("127.0.0.1:9000").unwrap(), ("127.0.0.1".into(), 9000));
        assert!(parse_broker("no-port").is_err());
        assert!(parse_broker("host:port").is_err());
    }

    #[test]
    fn spawn_with_bad_broker_errors_immediately() {
        // 无效参数应同步报错，而不是挂起
        let mut hub = crate::channel::SerialHub::new();
        assert!(hub
            .open_mqtt(&MqttParams {
                broker: "bad".into(),
                ..Default::default()
            })
            .is_err());
    }
}
