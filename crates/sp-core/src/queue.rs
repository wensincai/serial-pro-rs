//! 命令队列：顺序 + 延时 + 循环（对标 SuperCom AdvancedSend）。
//!
//! 运行状态机放在 UI 层（egui 单线程驱动），本模块只管数据与校验。
//! Windows `std::thread::sleep`/Timer 有 ~15ms 粒度误差，执行由 UI 帧驱动
//! （~30ms 重绘），比裸线程定时更稳，且可随时暂停。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub label: String,
    /// 原始内容（hex=true 按 Hex 解析，否则转义符展开）
    pub content: String,
    pub hex: bool,
    /// 本条发送后的延时（毫秒）
    pub delay_ms: u32,
}

/// 队列运行游标（UI 层持有）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QueueCursor {
    pub index: usize,
    /// 已完成的总轮数
    pub loops_done: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandQueue {
    pub items: Vec<QueueItem>,
    /// 循环轮数；0 = 无限循环
    pub loop_count: u32,
}

impl Default for CommandQueue {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            loop_count: 1,
        }
    }
}

impl CommandQueue {
    /// 取下一条目并推进游标；走完（含有限轮数用尽）返回 None。
    pub fn next(&self, cursor: &mut QueueCursor) -> Option<&QueueItem> {
        if self.items.is_empty() || self.finished(cursor) {
            return None;
        }
        let item = self.items.get(cursor.index)?;
        cursor.index += 1;
        if cursor.index >= self.items.len() {
            cursor.index = 0;
            cursor.loops_done += 1;
        }
        Some(item)
    }

    /// 判断游标是否已超出（用于走完后停止）。
    pub fn finished(&self, cursor: &QueueCursor) -> bool {
        if self.items.is_empty() {
            return true;
        }
        self.loop_count > 0
            && cursor.loops_done >= self.loop_count
            && cursor.index == 0
    }

    pub fn total_steps(&self) -> u64 {
        let per_loop = self.items.len() as u64;
        let loops = if self.loop_count == 0 {
            0 // 无限
        } else {
            self.loop_count as u64
        };
        per_loop * loops
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, delay: u32) -> QueueItem {
        QueueItem {
            label: label.into(),
            content: label.into(),
            hex: false,
            delay_ms: delay,
        }
    }

    #[test]
    fn finite_queue_walks_and_finishes() {
        let q = CommandQueue {
            items: vec![item("a", 10), item("b", 20)],
            loop_count: 2,
        };
        let mut c = QueueCursor { index: 0, loops_done: 0 };
        let seq: Vec<String> = std::iter::from_fn(|| q.next(&mut c).map(|i| i.label.clone())).collect();
        assert_eq!(seq, vec!["a", "b", "a", "b"]);
        assert!(q.finished(&c));
    }

    #[test]
    fn infinite_queue_never_finishes() {
        let q = CommandQueue {
            items: vec![item("a", 10)],
            loop_count: 0,
        };
        let mut c = QueueCursor { index: 0, loops_done: 0 };
        for _ in 0..1000 {
            assert!(q.next(&mut c).is_some());
        }
        assert!(!q.finished(&c));
    }

    #[test]
    fn empty_queue_finished() {
        let q = CommandQueue::default();
        let mut c = QueueCursor { index: 0, loops_done: 0 };
        assert!(q.next(&mut c).is_none());
        assert!(q.finished(&c));
    }

    #[test]
    fn serde_roundtrip() {
        let q = CommandQueue {
            items: vec![item("复位", 100)],
            loop_count: 3,
        };
        let s = serde_json::to_string(&q).unwrap();
        let back: CommandQueue = serde_json::from_str(&s).unwrap();
        assert_eq!(back, q);
    }
}
