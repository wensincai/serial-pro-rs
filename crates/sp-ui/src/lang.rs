//! 多语言支持：中/英文切换。
//!
//! 翻译以"中文原文 -> 英文"的映射表实现：`tr("打开串口")`，
//! 语言为中文时原样返回，英文时查表（缺条目回退中文原文）。
//! 新增条目：往 `EN` 表里加一条即可。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    pub fn from_code(code: &str) -> Self {
        match code {
            "en" => Lang::En,
            _ => Lang::Zh,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    pub fn other(&self) -> Self {
        match self {
            Lang::Zh => Lang::En,
            Lang::En => Lang::Zh,
        }
    }
}

/// 翻译：中文原文 -> 当前语言文本。键应为编译期字面量。
pub fn tr(lang: Lang, key: &'static str) -> &'static str {
    if lang == Lang::Zh {
        key
    } else {
        EN.iter()
            .find(|(zh, _)| *zh == key)
            .map(|(_, en)| *en)
            .unwrap_or(key)
    }
}

/// 英文翻译表：(中文原文, 英文)。
static EN: &[(&str, &str)] = &[
    // 通道类型
    ("串口", "Serial"),
    ("TCP客户端", "TCP Client"),
    ("TCP服务端", "TCP Server"),
    ("UDP", "UDP"),
    ("MQTT", "MQTT"),
    ("WinUSB", "WinUSB"),
    // 工具栏
    ("刷新端口", "Refresh"),
    ("目标", "Target"),
    ("监听", "Listen"),
    ("本地", "Local"),
    ("Broker", "Broker"),
    ("客户端ID", "Client ID"),
    ("订阅主题", "Sub topic"),
    ("发布主题", "Pub topic"),
    ("打开串口", "Open Port"),
    ("关闭串口", "Close Port"),
    ("打开连接", "Connect"),
    ("关闭连接", "Disconnect"),
    ("开始监听", "Start Listen"),
    ("停止监听", "Stop Listen"),
    ("打开 UDP", "Open UDP"),
    ("关闭 UDP", "Close UDP"),
    ("打开 MQTT", "Connect MQTT"),
    ("关闭 MQTT", "Disconnect MQTT"),
    ("打开 USB", "Open USB"),
    ("关闭 USB", "Close USB"),
    ("● 已打开", "● Open"),
    ("● 已连接", "● Connected"),
    ("○ 未打开", "○ Closed"),
    ("分包:", "Pkt ms:"),
    ("自动日志", "Auto Log"),
    ("MB/片", "MB/part"),
    // 发送区
    ("发送", "Send"),
    ("Hex 发送", "Hex Send"),
    ("Hex 显示", "Hex View"),
    ("ANSI 渲染", "ANSI Render"),
    ("暂停滚屏", "Pause Scroll"),
    ("清屏", "Clear"),
    ("转义:", "Esc:"),
    ("帧发送", "Frame"),
    ("定时发送", "Timer"),
    ("加入队列", "Add to Queue"),
    // 快捷发送
    ("快捷发送", "Quick Send"),
    ("发送历史", "History"),
    ("新建条目", "New Item"),
    ("全删", "Clear All"),
    // 命令队列
    ("命令队列", "Command Queue"),
    ("▶ 执行", "▶ Run"),
    ("■ 停止", "■ Stop"),
    ("循环:", "Loops:"),
    ("条", "items"),
    // 窗口标题
    ("Lua 双向钩子", "Lua Hooks"),
    ("示波器", "Oscilloscope"),
    ("串口监听（API Hook 注入）", "Serial Monitor (API Hook)"),
    ("应用", "Apply"),
    ("启用钩子", "Enable Hook"),
    ("脚本输出", "Script Output"),
    ("选择进程", "Pick Process"),
    ("COM 口号", "COM index"),
    ("开始监听注入", "Start Inject"),
    ("停止监听注入", "Stop Inject"),
    ("● 监听中", "● Monitoring"),
    ("○ 未监听", "○ Idle"),
    ("列", "Col"),
    ("清空", "Clear"),
    ("运行", "Run"),
    ("第1列", "Col 1"),
    ("第2列", "Col 2"),
    ("第3列", "Col 3"),
    // 状态栏
    ("RX", "RX"),
    ("TX", "TX"),
    ("字节", "bytes"),
    ("错误", "Error"),
    ("清除提示", "Dismiss"),
    // 消息
    ("没有已打开的串口", "No open channel"),
    ("语言", "Language"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_passthrough() {
        assert_eq!(tr(Lang::Zh, "打开串口"), "打开串口");
    }

    #[test]
    fn en_lookup() {
        assert_eq!(tr(Lang::En, "打开串口"), "Open Port");
        assert_eq!(tr(Lang::En, "命令队列"), "Command Queue");
    }

    #[test]
    fn en_fallback_to_zh() {
        assert_eq!(tr(Lang::En, " untranslated-key "), " untranslated-key ");
    }

    #[test]
    fn lang_cycle() {
        assert_eq!(Lang::Zh.other(), Lang::En);
        assert_eq!(Lang::En.other(), Lang::Zh);
        assert_eq!(Lang::from_code("en"), Lang::En);
        assert_eq!(Lang::from_code("zh"), Lang::Zh);
    }
}
