//! serial-pro —— 串口调试工作台（P0 MVP）
//!
//! 布局见 docs/02-架构设计.md 第 6 节。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui;
use sp_core::channel::{ChannelEvent, SerialHub, SerialId};
use sp_core::config::{Parity, PortConfig, Settings};
use sp_core::hexutil;

fn main() -> eframe::Result<()> {    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("serial-pro")
            .with_inner_size([1100.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "serial-pro",
        options,
        Box::new(|cc| {
            install_cjk_fonts(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}

// ---------------------------------------------------------------- 字体

/// 安装中文字体：egui 默认字体不含 CJK 字形（显示为方块）。
/// 从 Windows 系统字体目录加载，追加到 Proportional 与 Monospace 回退链尾部。
fn install_cjk_fonts(ctx: &egui::Context) {
    let candidates: [&str; 3] = ["msyh.ttc", "simhei.ttf", "simsun.ttc"];
    let mut loaded: Option<(String, egui::FontData)> = None;
    for name in candidates {
        let path = PathBuf::from(r"C:\Windows\Fonts").join(name);
        if let Ok(bytes) = std::fs::read(&path) {
            loaded = Some((format!("cjk-{name}"), egui::FontData::from_owned(bytes)));
            break;
        }
    }
    let Some((loaded_name, data)) = loaded else {
        return; // 非 Windows 或缺字体目录：保持默认
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(loaded_name.clone(), data.into());
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(loaded_name.clone());
    }
    ctx.set_fonts(fonts);
}

// ---------------------------------------------------------------- App

#[derive(Debug, Clone)]
struct RxRecord {
    ts: SystemTime,
    dir_tx: bool,
    bytes: Vec<u8>,
}

struct App {
    settings: Settings,
    settings_path: PathBuf,

    hub: SerialHub,
    /// 已打开的通道 -> 端口名映射（P0：每端口至多一个通道）
    port_channel: Vec<(SerialId, String)>,
    next_record_id: u64,
    records: VecDeque<RxRecord>,
    rx_bytes: u64,
    tx_bytes: u64,
    last_error: Option<String>,

    // 端口选择与参数
    ports: Vec<(String, String)>, // (端口名, 友好名)
    selected_port: usize,
    cfg: PortConfig,

    // 发送区
    send_text: String,
    send_history: Vec<String>,
    history_idx: Option<usize>,

    // 快捷发送编辑态
    quick_edit: Option<usize>, // 正在编辑的条目
    quick_label: String,
    quick_content: String,
    quick_hex: bool,

    scroll_paused: bool,
    scroll_to_bottom: bool,
}

impl App {
    fn new() -> Self {
        let settings_path = Settings::default_path();
        let settings = Settings::load(&settings_path);
        let mut app = Self {
            settings,
            settings_path,
            hub: SerialHub::new(),
            port_channel: Vec::new(),
            next_record_id: 0,
            records: VecDeque::new(),
            rx_bytes: 0,
            tx_bytes: 0,
            last_error: None,
            ports: Vec::new(),
            selected_port: 0,
            cfg: PortConfig::default(),
            send_text: String::new(),
            send_history: Vec::new(),
            history_idx: None,
            quick_edit: None,
            quick_label: String::new(),
            quick_content: String::new(),
            quick_hex: false,
            scroll_paused: false,
            scroll_to_bottom: true,
        };
        app.refresh_ports();
        if let Some((name, _)) = app.ports.first().cloned() {
            app.cfg = app.settings.port_config(&name);
        }
        app
    }

    fn refresh_ports(&mut self) {
        self.ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| {
                let mfg = match &p.port_type {
                    serialport::SerialPortType::UsbPort(info) => {
                        info.manufacturer.clone().unwrap_or_default()
                    }
                    _ => String::new(),
                };
                (p.port_name, mfg)
            })
            .collect();
    }

    fn selected_port_name(&self) -> Option<String> {
        self.ports.get(self.selected_port).map(|(n, _)| n.clone())
    }

    fn push_record(&mut self, dir_tx: bool, bytes: Vec<u8>) {
        if dir_tx {
            self.tx_bytes += bytes.len() as u64;
        } else {
            self.rx_bytes += bytes.len() as u64;
        }
        self.next_record_id += 1;
        self.records.push_back(RxRecord {
            ts: SystemTime::now(),
            dir_tx,
            bytes,
        });
        let max = self.cfg.max_records.max(100);
        while self.records.len() > max {
            self.records.pop_front();
        }
    }

    fn save_settings(&self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            eprintln!("保存设置失败: {e}");
        }
    }

    fn send_current(&mut self) {
        let text = self.send_text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let bytes = if self.cfg.hex_send {
            match hexutil::parse_hex(&text) {
                Ok(b) => b,
                Err(e) => {
                    self.last_error = Some(e.to_string());
                    return;
                }
            }
        } else {
            hexutil::unescape(&text)
        };
        if bytes.is_empty() {
            return;
        }
        // 发到所有已打开通道（P0 单口为主，多口同时发更顺手）
        let ids = self.hub.ids();
        if ids.is_empty() {
            self.last_error = Some("没有已打开的串口".into());
            return;
        }
        for id in ids {
            if let Some(a) = self.hub.get(id) {
                a.send_bytes(bytes.clone());
            }
        }
        self.push_record(true, bytes);
        if self.send_history.last().map(|s| s.as_str()) != Some(text.as_str()) {
            self.send_history.push(text.clone());
            if self.send_history.len() > 100 {
                self.send_history.remove(0);
            }
        }
        self.history_idx = None;
        self.send_text.clear();
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_channels();
        self.top_panel(ctx);
        self.right_panel(ctx);
        self.central_panel(ctx);
        self.bottom_panel(ctx);
        self.status_bar(ctx);
        // 持续重绘以轮询串口事件
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 记住当前端口配置
        if let Some(name) = self.selected_port_name() {
            self.cfg.name = name.clone();
            self.settings.ports.insert(name, self.cfg.clone());
        }
        self.save_settings();
    }
}

impl App {
    fn poll_channels(&mut self) {
        self.hub.reap_all();
        let ids = self.hub.ids();
        for id in ids {
            while let Some(ev) = self.hub.get(id).and_then(|a| a.try_recv()) {
                match ev {
                    ChannelEvent::Opened(_) => {}
                    ChannelEvent::Rx(_, bytes) => {
                        self.push_record(false, bytes);
                        self.scroll_to_bottom = true;
                    }
                    ChannelEvent::TxEcho(_, _) => { /* 发送时已手动记录 */ }
                    ChannelEvent::Closed(reason_id, reason) => {
                        if reason != "closed" {
                            self.last_error = Some(format!("端口 {} 已断开: {reason}", reason_id));
                        }
                        self.hub.remove_if_closed(reason_id);
                    }
                    ChannelEvent::Error(_, msg) => {
                        self.last_error = Some(msg);
                    }
                }
            }
        }
    }

    // -------------------------------------------------- 顶部工具栏
    fn top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("刷新端口").clicked() {
                    self.refresh_ports();
                }
                // 端口下拉（显示友好名）
                egui::ComboBox::from_id_salt("port_sel")
                    .width(220.0)
                    .selected_text(self.port_label(self.selected_port))
                    .show_ui(ui, |ui| {
                        for i in 0..self.ports.len() {
                            let label = self.port_label(i);
                            ui.selectable_value(&mut self.selected_port, i, label);
                        }
                    });

                // 波特率
                let bauds = [1200u32, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
                egui::ComboBox::from_id_salt("baud_sel")
                    .width(90.0)
                    .selected_text(format!("{}", self.cfg.baud))
                    .show_ui(ui, |ui| {
                        for b in bauds {
                            ui.selectable_value(&mut self.cfg.baud, b, format!("{b}"));
                        }
                    });
                if ui
                    .add(egui::DragValue::new(&mut self.cfg.baud).speed(100).prefix("自定:"))
                    .changed()
                {
                    // 自定义波特率直接编辑
                }

                // 数据位 / 校验 / 停止位
                egui::ComboBox::from_id_salt("db_sel")
                    .width(60.0)
                    .selected_text(format!("{}位", self.cfg.data_bits))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.cfg.data_bits, 7, "7位");
                        ui.selectable_value(&mut self.cfg.data_bits, 8, "8位");
                    });
                let mut parity = self.cfg.parity;
                egui::ComboBox::from_id_salt("parity_sel")
                    .width(60.0)
                    .selected_text(match parity {
                        Parity::None => "无校验",
                        Parity::Odd => "奇校验",
                        Parity::Even => "偶校验",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut parity, Parity::None, "无校验");
                        ui.selectable_value(&mut parity, Parity::Odd, "奇校验");
                        ui.selectable_value(&mut parity, Parity::Even, "偶校验");
                    });
                self.cfg.parity = parity;

                egui::ComboBox::from_id_salt("stop_sel")
                    .width(60.0)
                    .selected_text(format!("{}停止", self.cfg.stop_bits))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.cfg.stop_bits, 1, "1停止");
                        ui.selectable_value(&mut self.cfg.stop_bits, 2, "2停止");
                    });

                // 分包超时
                ui.add(
                    egui::DragValue::new(&mut self.cfg.pack_timeout_ms)
                        .speed(1)
                        .prefix("分包:"),
                )
                .on_hover_text("毫秒；<0 = 流模式（不分包）");

                // 打开/关闭
                let port_open = self
                    .selected_port_name()
                    .map(|n| self.port_channel.iter().any(|(_, p)| *p == n));

                // P0：一个端口只允许一个通道实例
                match port_open {
                    Some(true) => {
                        if ui.button("关闭串口").clicked() {
                            if let Some(id) = self.open_id_of_port() {
                                self.hub.close(id);
                            }
                        }
                        ui.colored_label(egui::Color32::from_rgb(60, 160, 60), "● 已打开");
                    }
                    _ => {
                        if ui.button("打开串口").clicked() {
                            self.open_selected_port();
                        }
                        ui.weak("○ 未打开");
                    }
                }
            });
        });
    }

    fn port_label(&self, i: usize) -> String {
        match self.ports.get(i) {
            Some((name, mfg)) if !mfg.is_empty() => format!("{name} — {mfg}"),
            Some((name, _)) => name.clone(),
            None => "(无端口)".into(),
        }
    }

    fn open_id_of_port(&self) -> Option<SerialId> {
        self.port_channel
            .iter()
            .find(|(_, p)| Some(p.as_str()) == self.selected_port_name().as_deref())
            .map(|(id, _)| *id)
    }

    fn open_selected_port(&mut self) {
        let Some(name) = self.selected_port_name() else {
            self.last_error = Some("没有可用串口".into());
            return;
        };
        if self.open_id_of_port().is_some() {
            return; // 已打开
        }
        self.cfg.name = name.clone();
        self.settings.ports.insert(name, self.cfg.clone());
        let cfg = self.cfg.clone();
        match self.hub.open(&cfg) {
            Ok(id) => {
                self.port_channel.push((id, cfg.name.clone()));
                self.save_settings();
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    // -------------------------------------------------- 右侧快捷发送
    fn right_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("quick_send")
            .resizable(true)
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.heading("快捷发送");
                ui.separator();
                if let Some(idx) = self.quick_edit {
                    // 编辑态
                    egui::Grid::new("qs_edit").num_columns(2).show(ui, |ui| {
                        ui.label("名称");
                        ui.text_edit_singleline(&mut self.quick_label);
                        ui.label("内容");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.quick_content)
                                .desired_rows(3)
                                .desired_width(220.0),
                        );
                        ui.label("Hex");
                        ui.checkbox(&mut self.quick_hex, "");
                    });
                    ui.horizontal(|ui| {
                        if ui.button("保存").clicked() {
                            let item = sp_core::config::QuickSendItem {
                                label: self.quick_label.clone(),
                                content: self.quick_content.clone(),
                                hex: self.quick_hex,
                            };
                            if idx < self.settings.quick_send.len() {
                                self.settings.quick_send[idx] = item;
                            } else {
                                self.settings.quick_send.push(item);
                            }
                            self.quick_edit = None;
                            self.save_settings();
                        }
                        if ui.button("取消").clicked() {
                            self.quick_edit = None;
                        }
                    });
                } else {
                    if ui.button("＋ 新增条目").clicked() {
                        self.quick_edit = Some(self.settings.quick_send.len());
                        self.quick_label.clear();
                        self.quick_content.clear();
                        self.quick_hex = self.cfg.hex_send;
                    }
                }
                ui.separator();

                let open_ids = self.hub.ids();
                let mut delete: Option<usize> = None;
                let mut edit: Option<usize> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, item) in self.settings.quick_send.clone().iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button(item.label.as_str()).clicked()
                                && !open_ids.is_empty()
                            {
                                let bytes = if item.hex {
                                    match hexutil::parse_hex(&item.content) {
                                        Ok(b) => b,
                                        Err(e) => {
                                            self.last_error = Some(e.to_string());
                                            return;
                                        }
                                    }
                                } else {
                                    hexutil::unescape(&item.content)
                                };
                                for id in &open_ids {
                                    if let Some(a) = self.hub.get(*id) {
                                        a.send_bytes(bytes.clone());
                                    }
                                }
                                self.push_record(true, bytes);
                            }
                            if ui.small_button("✎").clicked() {
                                edit = Some(i);
                            }
                            if ui.small_button("✕").clicked() {
                                delete = Some(i);
                            }
                        });
                        if !item.hex {
                            ui.weak(format!("  {}", item.content.replace('\n', "␊")));
                        }
                    }
                });
                if let Some(i) = delete {
                    self.settings.quick_send.remove(i);
                    self.save_settings();
                }
                if let Some(i) = edit {
                    let item = self.settings.quick_send.get(i).cloned();
                    if let Some(it) = item {
                        self.quick_edit = Some(i);
                        self.quick_label = it.label;
                        self.quick_content = it.content;
                        self.quick_hex = it.hex;
                    }
                }
            });
    }

    // -------------------------------------------------- 中央接收区
    fn central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.cfg.hex_view, "Hex 显示");
                ui.checkbox(&mut self.scroll_paused, "暂停滚屏");
                if ui.button("清屏").clicked() {
                    self.records.clear();
                }
                if ui.button("导出日志").clicked() {
                    self.export_log();
                }
            });
            ui.separator();

            let hex_view = self.cfg.hex_view;
            let total = self.records.len();
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(!self.scroll_paused)
                .show(ui, |ui| {
                    // 每帧最多渲染尾部 500 条，防大流量卡 UI
                    let skip = total.saturating_sub(500);
                    for rec in self.records.iter().skip(skip) {
                        let ts = rec
                            .ts
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|d| {
                                let secs = d.as_secs() % 86_400;
                                format!(
                                    "{:02}:{:02}:{:02}.{:03}",
                                    secs / 3600 + 8, // 本地 UTC+8 简化处理
                                    (secs % 3600) / 60,
                                    secs % 60,
                                    d.subsec_millis()
                                )
                            })
                            .unwrap_or_default();
                        let color = if rec.dir_tx {
                            egui::Color32::from_rgb(170, 60, 60)
                        } else {
                            egui::Color32::from_rgb(60, 140, 60)
                        };
                        let arrow = if rec.dir_tx { "→" } else { "←" };
                        let body = if hex_view {
                            hexutil::to_hex(&rec.bytes, " ")
                        } else {
                            hexutil::readable(&rec.bytes)
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.monospace(format!("{ts} {arrow}"));
                            ui.monospace(
                                egui::RichText::new(body).color(color).monospace(),
                            );
                        });
                    }
                });
        });
    }

    fn export_log(&mut self) {
        let dir = PathBuf::from("logs");
        let _ = std::fs::create_dir_all(&dir);
        let name = format!(
            "serial-pro-{}.log",
            std::time::SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );
        let path = dir.join(name);
        let mut out = String::new();
        for rec in &self.records {
            let dir_ch = if rec.dir_tx { "TX" } else { "RX" };
            out.push_str(&format!(
                "{} {} {}\n",
                dir_ch,
                hexutil::to_hex(&rec.bytes, " "),
                hexutil::readable(&rec.bytes).replace('\n', "\\n")
            ));
        }
        match std::fs::write(&path, out) {
            Ok(()) => self.last_error = Some(format!("已导出到 {}", path.display())),
            Err(e) => self.last_error = Some(format!("导出失败: {e}")),
        }
    }

    // -------------------------------------------------- 底部发送区
    fn bottom_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("send_bar").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.cfg.hex_send, "Hex");
                let send_clicked = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.send_text)
                            .desired_width(480.0)
                            .hint_text("输入内容，Enter 发送；Hex 模式如 AA 55；支持 \\r\\n\\x41"),
                    )
                    .lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("发送").clicked() || send_clicked {
                    self.send_current();
                }
                if ui.button("清空").clicked() {
                    self.send_text.clear();
                }
                ui.weak("↑↓ 历史");
            });
            ui.add_space(2.0);
        });
    }

    // -------------------------------------------------- 状态条
    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .show_separator_line(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("RX {} 字节", self.rx_bytes));
                    ui.separator();
                    ui.label(format!("TX {} 字节", self.tx_bytes));
                    ui.separator();
                    ui.label(format!("记录 {} 条", self.records.len()));
                    ui.separator();
                    ui.label(format!("打开 {} 个端口", self.hub.ids().len()));
                    if let Some(err) = &self.last_error {
                        ui.colored_label(egui::Color32::from_rgb(200, 80, 60), err);
                    }
                });
            });
    }
}
