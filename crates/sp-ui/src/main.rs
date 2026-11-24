//! serial-pro —— 串口调试工作台
//!
//! P0：串口 MVP。P1：日志模板+分片 / 快捷发送多页 / 帧结构发送 / 命令队列 / 定时发送。
//! 布局见 docs/02-架构设计.md 第 6 节。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use eframe::egui;
use sp_core::channel::{ChannelEvent, SerialHub, SerialId};
use sp_core::config::{Parity, PortConfig, QuickSendItem, Settings};
use sp_core::frame::{CrcAlgo, CrcRange};
use sp_core::logging::{LogWriter};
use sp_core::queue::{QueueCursor, QueueItem};
use sp_core::hexutil;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("serial-pro")
            .with_inner_size([1180.0, 780.0]),
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
        return;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportFmt {
    Text,
    Hex,
    Bin,
}

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
    /// 已打开的通道 -> 端口名映射（每端口至多一个通道）
    port_channel: Vec<(SerialId, String)>,
    records: VecDeque<RxRecord>,
    rx_bytes: u64,
    tx_bytes: u64,
    last_error: Option<String>,

    // 端口选择与参数
    ports: Vec<(String, String)>,
    selected_port: usize,
    cfg: PortConfig,

    // 接收显示
    scroll_paused: bool,

    // 发送区
    send_text: String,
    send_history: Vec<String>,
    history_idx: Option<usize>,

    // 快捷发送（多页）
    quick_page: usize,
    quick_edit: Option<usize>,
    quick_label: String,
    quick_content: String,
    quick_hex: bool,
    page_renaming: bool,

    // 帧结构发送
    frame_open: bool,

    // 命令队列
    queue_running: bool,
    queue_cursor: QueueCursor,
    queue_next_at: Option<Instant>,
    queue_edit: Option<usize>,
    queue_label: String,
    queue_content: String,
    queue_hex: bool,
    queue_delay_ms: u32,
    queue_loop_edit: u32,

    // 定时发送
    timer_enabled: bool,
    timer_last: Instant,

    // 日志
    log_writers: HashMap<SerialId, LogWriter>,
    export_fmt: ExportFmt,
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
            records: VecDeque::new(),
            rx_bytes: 0,
            tx_bytes: 0,
            last_error: None,
            ports: Vec::new(),
            selected_port: 0,
            cfg: PortConfig::default(),
            scroll_paused: false,
            send_text: String::new(),
            send_history: Vec::new(),
            history_idx: None,
            quick_page: 0,
            quick_edit: None,
            quick_label: String::new(),
            quick_content: String::new(),
            quick_hex: false,
            page_renaming: false,
            frame_open: false,
            queue_running: false,
            queue_cursor: QueueCursor { index: 0, loops_done: 0 },
            queue_next_at: None,
            queue_edit: None,
            queue_label: String::new(),
            queue_content: String::new(),
            queue_hex: false,
            queue_delay_ms: 100,
            queue_loop_edit: 1,
            timer_enabled: false,
            timer_last: Instant::now(),
            log_writers: HashMap::new(),
            export_fmt: ExportFmt::Text,
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

    /// 把字节发到所有已打开的通道。
    fn send_bytes_to_all(&mut self, bytes: &[u8]) {
        let ids = self.hub.ids();
        if ids.is_empty() {
            self.last_error = Some("没有已打开的串口".into());
            return;
        }
        for id in &ids {
            if let Some(a) = self.hub.get(*id) {
                a.send_bytes(bytes.to_vec());
            }
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
        self.send_bytes_to_all(&bytes);
        if self.send_history.last().map(|s| s.as_str()) != Some(text.as_str()) {
            self.send_history.push(text.clone());
            if self.send_history.len() > 100 {
                self.send_history.remove(0);
            }
        }
        self.history_idx = None;
        self.send_text.clear();
    }

    fn send_frame(&mut self) {
        let text = self.send_text.trim().to_string();
        if text.is_empty() {
            self.last_error = Some("帧载荷为空".into());
            return;
        }
        match sp_core::frame::build_from_text(&self.settings.frame, &text, self.cfg.hex_send) {
            Ok(bytes) => self.send_bytes_to_all(&bytes),
            Err(e) => self.last_error = Some(format!("组帧失败: {e}")),
        }
    }

    fn send_queue_item(&mut self, item: &QueueItem) {
        let bytes = if item.hex {
            match hexutil::parse_hex(&item.content) {
                Ok(b) => b,
                Err(e) => {
                    self.last_error = Some(format!("队列条目 {} Hex 错误: {e}", item.label));
                    return;
                }
            }
        } else {
            hexutil::unescape(&item.content)
        };
        self.send_bytes_to_all(&bytes);
    }

    // ---------------------------------------- 日志
    fn sync_log_writers(&mut self) {
        if self.settings.log.enabled {
            // 为每个打开的端口补建 writer
            for (id, port) in self.port_channel.clone() {
                if !self.log_writers.contains_key(&id) {
                    let w = LogWriter::new(self.settings.log.clone(), &port);
                    self.log_writers.insert(id, w);
                }
            }
        } else {
            self.log_writers.clear();
        }
    }

    fn log_write(&mut self, id: SerialId, dir_tx: bool, bytes: &[u8]) {
        if let Some(w) = self.log_writers.get_mut(&id) {
            if let Err(e) = w.write(dir_tx, bytes) {
                self.last_error = Some(format!("日志写入失败: {e}"));
            }
        }
    }

    fn open_selected_port(&mut self) {
        let Some(name) = self.selected_port_name() else {
            self.last_error = Some("没有可用串口".into());
            return;
        };
        if self.port_channel.iter().any(|(_, p)| *p == name) {
            return; // 已打开
        }
        self.cfg.name = name.clone();
        self.settings.ports.insert(name, self.cfg.clone());
        let cfg = self.cfg.clone();
        match self.hub.open(&cfg) {
            Ok(id) => {
                self.port_channel.push((id, cfg.name.clone()));
                if self.settings.log.enabled {
                    let w = LogWriter::new(self.settings.log.clone(), &cfg.name);
                    self.log_writers.insert(id, w);
                }
                self.save_settings();
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    fn close_port(&mut self, id: SerialId) {
        self.hub.close(id);
        self.log_writers.remove(&id);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_channels();
        self.tick_queue();
        self.tick_timer();
        self.top_panel(ctx);
        self.right_panel(ctx);
        self.central_panel(ctx);
        self.bottom_panel(ctx);
        self.status_bar(ctx);
        ctx.request_repaint_after(Duration::from_millis(30));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
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
        // 收发记录与日志统一由事件驱动
        for id in self.hub.ids() {
            while let Some(a) = self.hub.get(id) {
                let ev = match a.try_recv() {
                    Some(ev) => ev,
                    None => break,
                };
                match ev {
                    ChannelEvent::Opened(_) => {}
                    ChannelEvent::Rx(ch, bytes) => {
                        self.log_write(ch, false, &bytes);
                        self.push_record(false, bytes);
                    }
                    ChannelEvent::TxEcho(ch, bytes) => {
                        self.log_write(ch, true, &bytes);
                        self.push_record(true, bytes);
                    }
                    ChannelEvent::Closed(ch, reason) => {
                        if reason != "closed" {
                            self.last_error = Some(format!("端口已断开: {reason}"));
                        }
                        self.log_writers.remove(&ch);
                        self.hub.remove_if_closed(ch);
                    }
                    ChannelEvent::Error(_, msg) => {
                        self.last_error = Some(msg);
                    }
                }
            }
        }
    }

    // ---------------------------------------- 命令队列驱动
    fn tick_queue(&mut self) {
        if !self.queue_running {
            return;
        }
        let now = Instant::now();
        let Some(deadline) = self.queue_next_at else { return };
        if now < deadline {
            return;
        }
        // 队列数据借用：先取出再放回，避免借用冲突
        let q = std::mem::take(&mut self.settings.queue);
        let next = q.next(&mut self.queue_cursor).cloned();
        self.settings.queue = q;
        match next {
            Some(item) => {
                self.send_queue_item(&item);
                self.queue_next_at = Some(now + Duration::from_millis(item.delay_ms as u64));
            }
            None => {
                self.queue_running = false;
                self.queue_next_at = None;
                self.last_error = Some("命令队列执行完毕".into());
            }
        }
    }

    // ---------------------------------------- 定时发送驱动
    fn tick_timer(&mut self) {
        if !self.timer_enabled {
            return;
        }
        let interval = self.settings.timer_interval_ms.max(50);
        if self.timer_last.elapsed() >= Duration::from_millis(interval) {
            self.timer_last = Instant::now();
            self.send_current();
        }
    }

    // ---------------------------------------- 顶部工具栏
    fn top_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("刷新端口").clicked() {
                    self.refresh_ports();
                }
                egui::ComboBox::from_id_salt("port_sel")
                    .width(220.0)
                    .selected_text(self.port_label(self.selected_port))
                    .show_ui(ui, |ui| {
                        for i in 0..self.ports.len() {
                            let label = self.port_label(i);
                            ui.selectable_value(&mut self.selected_port, i, label);
                        }
                    });

                let bauds = [1200u32, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600];
                egui::ComboBox::from_id_salt("baud_sel")
                    .width(90.0)
                    .selected_text(format!("{}", self.cfg.baud))
                    .show_ui(ui, |ui| {
                        for b in bauds {
                            ui.selectable_value(&mut self.cfg.baud, b, format!("{b}"));
                        }
                    });
                ui.add(egui::DragValue::new(&mut self.cfg.baud).speed(100));

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

                ui.add(
                    egui::DragValue::new(&mut self.cfg.pack_timeout_ms)
                        .speed(1)
                        .prefix("分包:"),
                )
                .on_hover_text("毫秒；<0 = 流模式（不分包）");

                let port_open = self
                    .selected_port_name()
                    .map(|n| self.port_channel.iter().any(|(_, p)| *p == n));
                match port_open {
                    Some(true) => {
                        if ui.button("关闭串口").clicked() {
                            if let Some(id) = self.open_id_of_port() {
                                self.close_port(id);
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

                ui.separator();
                let mut log_on = self.settings.log.enabled;
                if ui.checkbox(&mut log_on, "自动日志").changed() {
                    self.settings.log.enabled = log_on;
                    self.sync_log_writers();
                    self.save_settings();
                }
                if log_on {
                    ui.weak(format!("{} MB/片", self.settings.log.frag_mb));
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

    // ---------------------------------------- 右侧：快捷发送 + 命令队列
    fn right_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("right")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                self.quick_send_ui(ui);
                ui.separator();
                self.queue_ui(ui);
            });
    }

    fn quick_send_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("快捷发送");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("＋页").clicked() && self.settings.quick_pages.len() < 10 {
                    let n = self.settings.quick_pages.len() + 1;
                    self.settings.quick_pages.push(Vec::new());
                    self.settings.quick_page_names.push(format!("第{n}页"));
                    self.save_settings();
                }
                if ui.small_button("－页").clicked() && self.settings.quick_pages.len() > 1 {
                    self.settings.quick_pages.pop();
                    self.settings.quick_page_names.pop();
                    self.quick_page = self.quick_page.min(self.settings.quick_pages.len() - 1);
                    self.save_settings();
                }
            });
        });

        // 页选择 + 重命名
        let page_count = self.settings.quick_pages.len();
        self.quick_page = self.quick_page.min(page_count - 1);
        ui.horizontal(|ui| {
            let names = self.settings.quick_page_names.clone();
            egui::ComboBox::from_id_salt("qs_page")
                .width(120.0)
                .selected_text(names.get(self.quick_page).cloned().unwrap_or_default())
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        ui.selectable_value(&mut self.quick_page, i, name.clone());
                    }
                });
            if ui.small_button("重命名").clicked() {
                self.page_renaming = !self.page_renaming;
            }
        });
        if self.page_renaming {
            ui.horizontal(|ui| {
                let mut name = self
                    .settings
                    .quick_page_names
                    .get(self.quick_page)
                    .cloned()
                    .unwrap_or_default();
                if ui.text_edit_singleline(&mut name).lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    self.settings.quick_page_names[self.quick_page] = name;
                    self.page_renaming = false;
                    self.save_settings();
                }
            });
        }
        ui.separator();

        // 编辑态 / 列表
        if let Some(idx) = self.quick_edit {
            egui::Grid::new("qs_edit").num_columns(2).show(ui, |ui| {
                ui.label("名称");
                ui.text_edit_singleline(&mut self.quick_label);
                ui.label("内容");
                ui.add(
                    egui::TextEdit::multiline(&mut self.quick_content)
                        .desired_rows(3)
                        .desired_width(240.0),
                );
                ui.label("Hex");
                ui.checkbox(&mut self.quick_hex, "");
            });
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    let item = QuickSendItem {
                        label: self.quick_label.clone(),
                        content: self.quick_content.clone(),
                        hex: self.quick_hex,
                    };
                    let page_len = self
                        .settings
                        .quick_pages
                        .get(self.quick_page)
                        .map(|p| p.len())
                        .unwrap_or(0);
                    if idx < page_len {
                        self.settings.quick_pages[self.quick_page][idx] = item;
                    } else {
                        self.settings.quick_pages[self.quick_page].push(item);
                    }
                    self.quick_edit = None;
                    self.save_settings();
                }
                if ui.button("取消").clicked() {
                    self.quick_edit = None;
                }
            });
        } else if ui.button("＋ 新增条目").clicked() {
            self.quick_edit = Some(
                self.settings
                    .quick_pages
                    .get(self.quick_page)
                    .map(|p| p.len())
                    .unwrap_or(0),
            );
            self.quick_label.clear();
            self.quick_content.clear();
            self.quick_hex = self.cfg.hex_send;
        }

        let mut delete: Option<usize> = None;
        let mut edit: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("qs_list")
            .max_height(220.0)
            .show(ui, |ui| {
                let page = self
                    .settings
                    .quick_pages
                    .get(self.quick_page)
                    .cloned()
                    .unwrap_or_default();
                for (i, item) in page.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button(item.label.as_str()).clicked() {
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
                            self.send_bytes_to_all(&bytes);
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
            if let Some(page) = self.settings.quick_pages.get_mut(self.quick_page) {
                if i < page.len() {
                    page.remove(i);
                    self.save_settings();
                }
            }
        }
        if let Some(i) = edit {
            if let Some(item) = self
                .settings
                .quick_pages
                .get(self.quick_page)
                .and_then(|p| p.get(i))
                .cloned()
            {
                self.quick_edit = Some(i);
                self.quick_label = item.label;
                self.quick_content = item.content;
                self.quick_hex = item.hex;
            }
        }
    }

    fn queue_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("命令队列");
        });
        ui.horizontal(|ui| {
            ui.label("循环");
            let mut loop_count = self.settings.queue.loop_count;
            if ui
                .add(
                    egui::DragValue::new(&mut loop_count)
                        .range(0..=999)
                        .suffix("轮"),
                )
                .changed()
            {
                self.settings.queue.loop_count = loop_count;
                self.queue_loop_edit = loop_count;
                self.save_settings();
            }
            ui.weak("0=无限");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.queue_running {
                    if ui.button("■ 停止").clicked() {
                        self.queue_running = false;
                        self.queue_next_at = None;
                    }
                } else if ui.button("▶ 执行").clicked() {
                    if self.settings.queue.items.is_empty() {
                        self.last_error = Some("队列为空".into());
                    } else {
                        self.queue_cursor = QueueCursor { index: 0, loops_done: 0 };
                        self.queue_running = true;
                        self.queue_next_at = Some(Instant::now()); // 立即发第一条
                    }
                }
            });
        });
        // 进度
        if self.queue_running {
            let remain_ms = self
                .queue_next_at
                .map(|d| d.saturating_duration_since(Instant::now()).as_millis())
                .unwrap_or(0);
            ui.weak(format!(
                "运行中: 第 {} 条 (第 {} 轮), 剩余 {remain_ms} ms",
                self.queue_cursor.index + 1,
                self.queue_cursor.loops_done + 1,
            ));
        }

        // 队列编辑态
        if let Some(idx) = self.queue_edit {
            egui::Grid::new("qq_edit").num_columns(2).show(ui, |ui| {
                ui.label("名称");
                ui.text_edit_singleline(&mut self.queue_label);
                ui.label("内容");
                ui.add(
                    egui::TextEdit::multiline(&mut self.queue_content)
                        .desired_rows(2)
                        .desired_width(240.0),
                );
                ui.label("Hex");
                ui.checkbox(&mut self.queue_hex, "");
                ui.label("延时");
                ui.add(egui::DragValue::new(&mut self.queue_delay_ms).suffix(" ms"));
            });
            ui.horizontal(|ui| {
                if ui.button("保存").clicked() {
                    let item = QueueItem {
                        label: self.queue_label.clone(),
                        content: self.queue_content.clone(),
                        hex: self.queue_hex,
                        delay_ms: self.queue_delay_ms,
                    };
                    if idx < self.settings.queue.items.len() {
                        self.settings.queue.items[idx] = item;
                    } else {
                        self.settings.queue.items.push(item);
                    }
                    self.queue_edit = None;
                    self.save_settings();
                }
                if ui.button("取消").clicked() {
                    self.queue_edit = None;
                }
            });
        } else if ui.button("＋ 加入队列").clicked() {
            let content = self.send_text.trim().to_string();
            if content.is_empty() {
                self.last_error = Some("发送框为空，先输入内容再加入队列".into());
            } else {
                self.queue_edit = Some(self.settings.queue.items.len());
                self.queue_label = format!("命令{}", self.settings.queue.items.len() + 1);
                self.queue_content = content;
                self.queue_hex = self.cfg.hex_send;
                self.queue_delay_ms = 100;
            }
        }

        let mut delete: Option<usize> = None;
        let mut edit: Option<usize> = None;
        egui::ScrollArea::vertical()
            .id_salt("qq_list")
            .max_height(220.0)
            .show(ui, |ui| {
                let items = self.settings.queue.items.clone();
                for (i, item) in items.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.monospace(format!("{}.", i + 1));
                        ui.label(item.label.as_str());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                delete = Some(i);
                            }
                            if ui.small_button("✎").clicked() {
                                edit = Some(i);
                            }
                            let mut delay = item.delay_ms;
                            if ui
                                .add(egui::DragValue::new(&mut delay).suffix("ms").speed(10))
                                .changed()
                            {
                                if let Some(it) = self.settings.queue.items.get_mut(i) {
                                    it.delay_ms = delay;
                                    self.save_settings();
                                }
                            }
                        });
                    });
                }
            });
        if let Some(i) = delete {
            if i < self.settings.queue.items.len() {
                self.settings.queue.items.remove(i);
                self.save_settings();
            }
        }
        if let Some(i) = edit {
            if let Some(item) = self.settings.queue.items.get(i).cloned() {
                self.queue_edit = Some(i);
                self.queue_label = item.label;
                self.queue_content = item.content;
                self.queue_hex = item.hex;
                self.queue_delay_ms = item.delay_ms;
            }
        }
    }

    // ---------------------------------------- 中央接收区
    fn central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.cfg.hex_view, "Hex 显示");
                ui.checkbox(&mut self.scroll_paused, "暂停滚屏");
                if ui.button("清屏").clicked() {
                    self.records.clear();
                }
                ui.separator();
                let mut fmt = self.export_fmt;
                egui::ComboBox::from_id_salt("export_fmt")
                    .width(90.0)
                    .selected_text(match fmt {
                        ExportFmt::Text => "文本",
                        ExportFmt::Hex => "Hex",
                        ExportFmt::Bin => "Bin",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut fmt, ExportFmt::Text, "文本");
                        ui.selectable_value(&mut fmt, ExportFmt::Hex, "Hex");
                        ui.selectable_value(&mut fmt, ExportFmt::Bin, "Bin");
                    });
                self.export_fmt = fmt;
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
                    let skip = total.saturating_sub(500);
                    for rec in self.records.iter().skip(skip) {
                        let ts = fmt_ts(rec.ts);
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
                            ui.monospace(egui::RichText::new(body).color(color).monospace());
                        });
                    }
                });
        });
    }

    fn export_log(&mut self) {
        let dir = PathBuf::from("logs");
        let _ = std::fs::create_dir_all(&dir);
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        match self.export_fmt {
            ExportFmt::Bin => {
                let mut raw = Vec::new();
                for rec in &self.records {
                    if !rec.dir_tx {
                        raw.extend(&rec.bytes);
                    }
                }
                let path = dir.join(format!("serial-pro-{stamp}.bin"));
                match std::fs::write(&path, raw) {
                    Ok(()) => self.last_error = Some(format!("已导出 {}", path.display())),
                    Err(e) => self.last_error = Some(format!("导出失败: {e}")),
                }
            }
            ExportFmt::Text | ExportFmt::Hex => {
                let mut out = String::new();
                for rec in &self.records {
                    let dir_ch = if rec.dir_tx { "TX" } else { "RX" };
                    let body = match self.export_fmt {
                        ExportFmt::Hex => hexutil::to_hex(&rec.bytes, " "),
                        _ => hexutil::readable(&rec.bytes).replace('\n', "\\n"),
                    };
                    let ts = fmt_ts_full(rec.ts);
                    out.push_str(&format!("{ts} [{dir_ch}] {body}\n"));
                }
                let ext = if matches!(self.export_fmt, ExportFmt::Hex) { "hex" } else { "log" };
                let path = dir.join(format!("serial-pro-{stamp}.{ext}"));
                match std::fs::write(&path, out) {
                    Ok(()) => self.last_error = Some(format!("已导出 {}", path.display())),
                    Err(e) => self.last_error = Some(format!("导出失败: {e}")),
                }
            }
        }
    }

    // ---------------------------------------- 底部发送区
    fn bottom_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("send_bar").show(ctx, |ui| {
            ui.add_space(4.0);

            // 帧发送行（可折叠）
            ui.horizontal(|ui| {
                ui.toggle_value(&mut self.frame_open, "帧发送");
                if self.frame_open {
                    ui.label("帧头");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.frame.header)
                            .desired_width(90.0)
                            .hint_text("AA 55"),
                    );
                    ui.label("校验");
                    egui::ComboBox::from_id_salt("crc_sel")
                        .width(110.0)
                        .selected_text(crc_label(self.settings.frame.crc))
                        .show_ui(ui, |ui| {
                            for a in [
                                CrcAlgo::None,
                                CrcAlgo::Sum8,
                                CrcAlgo::Xor,
                                CrcAlgo::Lrc,
                                CrcAlgo::Crc16Modbus,
                                CrcAlgo::Crc16Ccitt,
                                CrcAlgo::Crc16Xmodem,
                            ] {
                                ui.selectable_value(&mut self.settings.frame.crc, a, crc_label(a));
                            }
                        });
                    if self.settings.frame.crc != CrcAlgo::None {
                        let mut range = self.settings.frame.crc_range;
                        egui::ComboBox::from_id_salt("crc_range")
                            .width(110.0)
                            .selected_text(match range {
                                CrcRange::Payload => "校验:载荷",
                                CrcRange::HeaderPayload => "校验:帧头+载荷",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut range, CrcRange::Payload, "校验:载荷");
                                ui.selectable_value(&mut range, CrcRange::HeaderPayload, "校验:帧头+载荷");
                            });
                        self.settings.frame.crc_range = range;
                    }
                    ui.label("帧尾");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.frame.tail)
                            .desired_width(70.0)
                            .hint_text("0D 0A"),
                    );
                    if ui.button("按帧发送").clicked() {
                        self.send_frame();
                    }
                    if ui.small_button("存设置").clicked() {
                        self.save_settings();
                    }
                }
            });
            if self.frame_open {
                ui.weak("载荷取发送框内容；帧结构 = 帧头 + 载荷 + 校验 + 帧尾");
            }
            ui.separator();

            // 发送 + 定时行
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.cfg.hex_send, "Hex");
                let send_clicked = ui
                    .add(
                        egui::TextEdit::singleline(&mut self.send_text)
                            .desired_width(420.0)
                            .hint_text("Enter 发送；Hex 如 AA 55；支持 \\r\\n\\x41"),
                    )
                    .lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("发送").clicked() || send_clicked {
                    self.send_current();
                }
                if ui.button("清空").clicked() {
                    self.send_text.clear();
                }
                ui.separator();
                ui.checkbox(&mut self.timer_enabled, "定时");
                let mut interval = self.settings.timer_interval_ms;
                if ui
                    .add(egui::DragValue::new(&mut interval).suffix("ms").speed(100))
                    .changed()
                {
                    self.settings.timer_interval_ms = interval.max(50);
                    self.save_settings();
                }
                if self.timer_enabled {
                    let remain = self
                        .settings
                        .timer_interval_ms
                        .max(50)
                        .saturating_sub(self.timer_last.elapsed().as_millis() as u64);
                    ui.weak(format!("剩 {remain}ms"));
                }
                ui.separator();
                ui.weak("↑↓ 历史");
            });
            ui.add_space(2.0);
        });
    }

    // ---------------------------------------- 状态条
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
                    if self.settings.log.enabled && !self.log_writers.is_empty() {
                        ui.colored_label(egui::Color32::from_rgb(90, 140, 200), "日志记录中");
                    }
                    if let Some(err) = &self.last_error {
                        ui.colored_label(egui::Color32::from_rgb(200, 80, 60), err);
                    }
                    if ui.small_button("清除提示").clicked() {
                        self.last_error = None;
                    }
                });
            });
    }
}

// ---------------------------------------------------------------- 工具

fn crc_label(a: CrcAlgo) -> &'static str {
    match a {
        CrcAlgo::None => "无",
        CrcAlgo::Sum8 => "Sum8",
        CrcAlgo::Xor => "XOR",
        CrcAlgo::Lrc => "LRC",
        CrcAlgo::Crc16Modbus => "CRC16/MODBUS",
        CrcAlgo::Crc16Ccitt => "CRC16/CCITT",
        CrcAlgo::Crc16Xmodem => "CRC16/XMODEM",
    }
}

/// 时间戳 HH:MM:SS.mmm（本地 UTC+8）
fn fmt_ts(t: SystemTime) -> String {
    let (_, _, _, h, mi, s, ms) = sp_core::logging::split_ts(t);
    format!("{h:02}:{mi:02}:{s:02}.{ms:03}")
}

/// 完整日期时间 YYYY-MM-DD HH:MM:SS.mmm
fn fmt_ts_full(t: SystemTime) -> String {
    let (y, m, d, h, mi, s, ms) = sp_core::logging::split_ts(t);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02}.{ms:03}")
}
