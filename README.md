# serial-pro-rs

串口调试工作台（Rust + egui）。产品定位：**串口本职体验（SuperCom/sscom 级）+ 可编程脚本平台（llcom 通道架构）的融合体**。

参照项目对比与选型依据见 `docs/01-需求分析.md`。

## 特性

### P0 串口 MVP
- 串口收发：文本/Hex 显示、文本/Hex 发送、转义符（`\r\n\t\xNN\0`）
- 分包超时可调（聚包窗口 / 负数流模式），按包显示时间戳与方向
- 每端口参数记忆（重开如旧，settings.json 持久化 + .bakup 恢复）
- 发送历史、RX/TX 计数、接收区环形缓冲（大流量不冻结）
- 单 exe 免安装；CJK 界面字体

### P1 数据与自动化
- 自动日志：文件名/目录模板（`%C %Y %M %D %h %m %s %fff`）、按 MB 分片、TX/RX 行前缀
- 日志三态导出：文本 / Hex / Bin（原始接收字节）
- 快捷发送多页（最多 10 页，页面重命名/增删）
- 帧结构发送：帧头 + 载荷 + 校验（Sum8/XOR/LRC/CRC16-MODBUS/CCITT-FALSE/XMODEM）+ 帧尾，校验范围可选
- 命令队列：顺序 + 每条延时 + 循环轮数（0=无限），运行进度实时显示
- 定时发送：可调周期（≥50ms）自动重发发送框内容

### P2 可编程与网络通道
- **Lua 双向钩子**（mlua / Lua 5.4）：`on_send`/`on_recv` 函数替换收发数据，返回 nil 透传；
  工具 `sp.hex`/`sp.fromhex`/`sp.log`，脚本内置编辑器与输出面板，脚本错误不影响透传
- **通用通道**：TCP 客户端 / TCP 服务端（多客户端并发）/ UDP，与串口共用一套收发 UI
- **示波器**：从接收数据自动提取数值列（文本数字 / Hex 字节），3 色曲线实时滚动

### P3 监听与终端
- **串口监听**（来自 llcom serial_monitor_rs，Apache-2.0）：注入目标进程 Hook
  ReadFile/WriteFile，监视任意进程对指定 COM 口的收发，数据汇入主接收区
  （需管理员权限，仅支持 64 位目标进程）
- **ANSI 终端渲染**：SGR 颜色（16 色/256 色/真彩）、粗体/斜体/下划线；
  光标/清屏序列剥离；适合 ESP32/STM32 等 boot log 调试

## 构建

```bat
cargo build --release        :: 产物 target\release\serial-pro.exe
cargo test                   :: 全部单元测试
```

## 路线图

| 阶段 | 内容 |
|---|---|
| P0 | 串口 MVP ✅ |
| P1 | 日志模板+分片、快捷发送多页、帧结构发送、命令队列、定时发送 ✅ |
| P2 | Lua 双向钩子、TCP/UDP 通道、示波器 ✅ |
| P3 | 串口监听（API Hook 注入）、ANSI 终端 ✅ |
| P4 | NSIS 安装包、多语言、MQTT 通道、WinUSB |

## 工程结构

```
crates/sp-core              引擎：串口/网络通道、Lua 钩子、示波器供数、ANSI、日志 —— 无 UI 依赖，可测试
crates/sp-ui                egui 界面
crates/serial_monitor       串口监听注入器（llcom serial_monitor_rs，Apache-2.0）
crates/serial_monitor_hook  注入用 Hook DLL（hook ReadFile/WriteFile）
docs/                       需求分析 / 架构设计 / 开发计划
```

## License

MIT（serial_monitor 与 serial_monitor_hook 两个 crate 及其源码遵循 Apache-2.0，
来自 llcom 项目，见各 crate 目录内 LICENSE 文件）
