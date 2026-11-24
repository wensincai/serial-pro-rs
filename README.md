# serial-pro-rs

串口调试工作台（Rust + egui）。产品定位：**串口本职体验（SuperCom/sscom 级）+ 可编程脚本平台（llcom 通道架构）的融合体**。

参照项目对比与选型依据见 `docs/01-需求分析.md`。

## 特性（P0）

- 串口收发：文本/Hex 显示、文本/Hex 发送、转义符（`\r\n\t\xNN\0`）
- 分包超时可调（聚包窗口 / 负数流模式），按包显示时间戳与方向
- 快捷发送条目（即点即发、内联编辑）
- 每端口参数记忆（重开如旧，settings.json 持久化 + .bakup 恢复）
- 发送历史（上下键回溯）、RX/TX 计数、日志导出
- 单 exe 免安装；接收区环形缓冲，大流量不冻结

## 构建

```bat
cargo build --release        :: 产物 target\release\serial-pro.exe
cargo test                   :: sp-core 单元测试
```

## 路线图

| 阶段 | 内容 |
|---|---|
| P0 | 串口 MVP（当前） |
| P1 | 日志模板+分片、快捷发送多页、帧结构发送（帧头+校验+帧尾）、命令队列、定时发送 |
| P2 | Lua 双向钩子（mlua）、通用通道（TCP/UDP/MQTT）、曲线/示波器 |
| P3 | 串口监听（复用 llcom serial_monitor_rs）、WinUSB、ANSI 终端 |
| P4 | NSIS 安装包、多语言 |

## 工程结构

```
crates/sp-core   引擎：串口工作线程（聚包）、通道协议、配置 —— 无 UI 依赖，可测试
crates/sp-ui     egui 界面
docs/            需求分析 / 架构设计 / 开发计划
```

## License

MIT
