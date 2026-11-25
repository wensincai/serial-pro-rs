//! WinUSB 通道：libusb（rusb，vendored 静态编译）。
//!
//! 工作模型：
//! - 按 VID/PID 打开设备，自动挑选第一个含 bulk IN/OUT 端点的接口；
//! - 收：bulk IN 读（20ms 超时轮询）-> Rx；
//! - 发：bulk OUT 写 -> TxEcho；
//! - 附加能力：设备枚举（`list_winusb_devices`）供 UI 下拉选择。

use std::time::Duration;

use crossbeam_channel as mpmc;
use rusb::{DeviceHandle, UsbContext};

use crate::channel::{ActiveSerial, ChannelCommand, ChannelEvent, SerialId};
use crate::error::{Error, Result};

/// 收发轮询超时
const XFER_TIMEOUT: Duration = Duration::from_millis(20);

/// 可访问的 WinUSB/libusb 设备摘要（供 UI 列表）。
#[derive(Debug, Clone)]
pub struct UsbDeviceDesc {
    pub vid: u16,
    pub pid: u16,
    /// 制造商/产品描述（拿不到时为空）
    pub label: String,
    /// 找到的 bulk 端点：(in_ep, out_ep)，None 表示该设备没有 bulk 端点
    pub endpoints: Option<(u8, u8)>,
}

/// 枚举当前可访问的 USB 设备（调试用，可能含非 WinUSB 设备）。
pub fn list_winusb_devices() -> Vec<UsbDeviceDesc> {
    let mut out = Vec::new();
    let Ok(api) = rusb::Context::new() else {
        return out;
    };
    let Ok(list) = api.devices() else { return out };
    for dev in list.iter() {
        let Ok(desc) = dev.device_descriptor() else {
            continue;
        };
        let (vid, pid) = (desc.vendor_id(), desc.product_id());
        let label = dev.open().map(|h| {
            let m = h.read_manufacturer_string_ascii(&desc).ok();
            let p = h.read_product_string_ascii(&desc).ok();
            match (m, p) {
                (Some(m), Some(p)) => format!("{m} {p}"),
                (Some(m), None) => m,
                (None, Some(p)) => p,
                (None, None) => String::new(),
            }
        }).unwrap_or_default();
        let endpoints = dev
            .open()
            .ok()
            .and_then(|h| find_bulk_pair(&h).ok())
            .map(|(_, i, o)| (i, o));
        out.push(UsbDeviceDesc {
            vid,
            pid,
            label,
            endpoints,
        });
    }
    out
}

/// 在设备配置里找第一个同时有 bulk IN + bulk OUT 端点的 (interface, in_ep, out_ep)。
fn find_bulk_pair<T: UsbContext>(
    handle: &DeviceHandle<T>,
) -> Result<(u8, u8, u8)> {
    let dev = handle.device();
    let desc = match dev.device_descriptor() {
        Ok(d) => d,
        Err(e) => return Err(Error::InvalidParam(format!("读取设备描述失败: {e}"))),
    };
    for i in 0..desc.num_configurations() {
        let Ok(cfg) = dev.config_descriptor(i) else { continue };
        for iface in cfg.interfaces() {
            for setting in iface.descriptors() {
                let mut in_ep = None;
                let mut out_ep = None;
                for ep in setting.endpoint_descriptors() {
                    if ep.transfer_type() == rusb::TransferType::Bulk {
                        match ep.direction() {
                            rusb::Direction::In => in_ep = Some(ep.address()),
                            rusb::Direction::Out => out_ep = Some(ep.address()),
                        }
                    }
                }
                if let (Some(i), Some(o)) = (in_ep, out_ep) {
                    return Ok((setting.interface_number(), i, o));
                }
            }
        }
    }
    Err(Error::InvalidParam("设备没有可用的 bulk IN/OUT 端点对".into()))
}

/// 启动 WinUSB 通道 worker。
pub fn spawn_winusb(id: SerialId, vid: u16, pid: u16) -> Result<ActiveSerial> {
    spawn_worker(id, format!("usb:{vid:04X}:{pid:04X}"), move |cmd_rx, event_tx| {
        let Ok(ctx) = rusb::Context::new() else {
            let _ = event_tx.send(ChannelEvent::Error(id, "libusb 初始化失败".into()));
            finish(id, &event_tx, false);
            return;
        };
        let Ok(list) = ctx.devices() else {
            let _ = event_tx.send(ChannelEvent::Error(id, "枚举 USB 设备失败".into()));
            finish(id, &event_tx, false);
            return;
        };
        let Some(dev) = list.iter().find(|d| {
            d.device_descriptor()
                .map(|desc| desc.vendor_id() == vid && desc.product_id() == pid)
                .unwrap_or(false)
        }) else {
            let _ = event_tx.send(ChannelEvent::Error(
                id,
                format!("未找到 USB 设备 {vid:04X}:{pid:04X}（检查设备是否插入、驱动是否为 WinUSB/libusb）"),
            ));
            finish(id, &event_tx, false);
            return;
        };
        let handle = match dev.open() {
            Ok(h) => h,
            Err(e) => {
                let _ = event_tx.send(ChannelEvent::Error(
                    id,
                    format!("打开设备失败: {e}（Windows 需安装 WinUSB/libusb 驱动）"),
                ));
                finish(id, &event_tx, false);
                return;
            }
        };
        let (iface, in_ep, out_ep) = match find_bulk_pair(&handle) {
            Ok(v) => v,
            Err(e) => {
                let _ = event_tx.send(ChannelEvent::Error(id, e.to_string()));
                finish(id, &event_tx, false);
                return;
            }
        };
        // 内核驱动占用时分离（Linux 常见；Windows 下 WinUSB 驱动无需此步）
        if handle.kernel_driver_active(iface).unwrap_or(false) {
            let _ = handle.detach_kernel_driver(iface);
        }
        if let Err(e) = handle.claim_interface(iface) {
            let _ = event_tx
                .send(ChannelEvent::Error(id, format!("占用接口 {iface} 失败: {e}")));
            finish(id, &event_tx, false);
            return;
        }
        let _ = event_tx.send(ChannelEvent::Opened(id));

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| loop {
            match cmd_rx.try_recv() {
                Ok(ChannelCommand::Close) | Err(mpmc::TryRecvError::Disconnected) => break,
                Ok(ChannelCommand::Send(bytes)) => match handle.write_bulk(out_ep, &bytes, XFER_TIMEOUT) {
                    Ok(_) => {
                        let _ = event_tx.send(ChannelEvent::TxEcho(id, bytes));
                    }
                    Err(e) => {
                        let _ =
                            event_tx.send(ChannelEvent::Error(id, format!("bulk 写失败: {e}")));
                    }
                },
                Err(mpmc::TryRecvError::Empty) => {}
            }
            let mut buf = [0u8; 4096];
            match handle.read_bulk(in_ep, &mut buf, XFER_TIMEOUT) {
                Ok(n) if n > 0 => {
                    let _ = event_tx.send(ChannelEvent::Rx(id, buf[..n].to_vec()));
                }
                Ok(_) => {}
                Err(rusb::Error::Timeout) => {}
                Err(e) => {
                    let _ =
                        event_tx.send(ChannelEvent::Error(id, format!("bulk 读失败: {e}")));
                    break;
                }
            }
        }))
        .is_err();
        let _ = handle.release_interface(iface);
        finish(id, &event_tx, panic);
    })
}

fn spawn_worker<F>(id: SerialId, name: String, f: F) -> Result<ActiveSerial>
where
    F: FnOnce(mpmc::Receiver<ChannelCommand>, mpmc::Sender<ChannelEvent>) + Send + 'static,
{
    let (cmd_tx, cmd_rx) = mpmc::unbounded::<ChannelCommand>();
    let (event_tx, event_rx) = mpmc::unbounded::<ChannelEvent>();
    let join = std::thread::Builder::new()
        .name(format!("sp-usb-{id}-{name}"))
        .spawn(move || f(cmd_rx, event_tx))
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
    Ok(ActiveSerial::new(id, cmd_tx, event_rx, join))
}

fn finish(id: SerialId, event_tx: &mpmc::Sender<ChannelEvent>, panic: bool) {
    if panic {
        let _ = event_tx.send(ChannelEvent::Error(id, "USB 线程 panic".into()));
    }
    let _ = event_tx.send(ChannelEvent::Closed(id, "closed".into()));
}

/// 解析 `VID:PID` 十六进制串（支持 `0x` 前缀、大小写混排）。
pub fn parse_vid_pid(s: &str) -> Result<(u16, u16)> {
    fn strip(t: &str) -> &str {
        t.trim().trim_start_matches("0x").trim_start_matches("0X")
    }
    let s = strip(s);
    let (vid, pid) = s
        .split_once([':', '-'])
        .ok_or_else(|| Error::InvalidParam(format!("VID:PID 格式无效: {s}")))?;
    let (vid, pid) = (strip(vid), strip(pid));
    let vid = u16::from_str_radix(vid, 16)
        .map_err(|_| Error::InvalidParam(format!("VID 无效: {vid}")))?;
    let pid = u16::from_str_radix(pid, 16)
        .map_err(|_| Error::InvalidParam(format!("PID 无效: {pid}")))?;
    Ok((vid, pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vid_pid_parsing() {
        assert_eq!(parse_vid_pid("1A86:7523").unwrap(), (0x1A86, 0x7523));
        assert_eq!(parse_vid_pid("0x1a86-0x7523").unwrap(), (0x1A86, 0x7523));
        assert_eq!(parse_vid_pid("  2E8A : 0003  ").unwrap(), (0x2E8A, 3));
        assert!(parse_vid_pid("nope").is_err());
        assert!(parse_vid_pid("ZZ:11").is_err());
    }

    #[test]
    fn enumeration_does_not_panic() {
        // 无设备环境下只要求安全返回
        let _ = list_winusb_devices();
    }
}
