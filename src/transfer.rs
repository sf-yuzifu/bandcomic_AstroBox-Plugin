//! 滑动窗口 + 累计 ACK 的可靠传输状态机。
//!
//! 移植自 InterconnectFetch 插件 transfer.rs（v3），泛化为双向复用：
//! 插件→手表（漫画导入上传，`WindowedSender`）与手表→插件（app_data/封面
//! 同步，`RecvFrontier`）。帧格式与业务语义留在调用方，这里只维护序号状态。
//!
//! 语义约定：
//! - 发送方任一时刻最多 `window` 帧在途；接收方每收一帧回一个累计 ACK
//!   （`ack` = 下一个仍缺失的连续序号，即 `seq < ack` 全部已收）。
//! - ACK 前进 → 窗口前移补发新帧；ACK 停滞（重复 ACK）→ go-back-N 整窗重发，
//!   每个停滞点只重传一次（`retx_base` 防止单次丢片的一串重复 ACK 引发重传风暴）。
//! - 不依赖底层有序：接收方按 gseq 乱序缓存、按连续前沿顺序消费，
//!   安卓端 QAIC 消息乱序因此不再是问题。

use serde_json::Value;
use std::collections::HashMap;

/// 发送方滑动窗口状态机（纯状态，不做 I/O；发送动作由调用方在锁外执行）
#[derive(Debug, Clone)]
pub struct WindowedSender {
    window: usize,
    /// 首个未确认帧序号 = 对端累计 ACK 值
    base: usize,
    /// 下一个待发帧序号
    next: usize,
    /// 最近一次 go-back-N 的停滞点；base 单调递增，保证同一停滞点只重传一次
    retx_base: Option<usize>,
}

impl WindowedSender {
    pub fn new(window: usize) -> Self {
        Self {
            window: window.max(1),
            base: 0,
            next: 0,
            retx_base: None,
        }
    }

    /// 当前累计确认前沿（= 已确认帧数）
    pub fn base(&self) -> usize {
        self.base
    }

    /// 收集窗口内待发帧序号：[next, min(base+window, total))
    pub fn pump(&mut self, total: usize) -> Vec<usize> {
        let mut out = Vec::new();
        while self.next < self.base + self.window && self.next < total {
            out.push(self.next);
            self.next += 1;
        }
        out
    }

    /// 处理累计 ACK：前进则补发新帧；停滞则整窗 go-back-N 重发（每停滞点一次）。
    /// 返回 (待发帧序号, 是否全部确认完成)
    pub fn on_ack(&mut self, ack: usize, total: usize) -> (Vec<usize>, bool) {
        let ack = ack.min(total);
        if ack > self.base {
            self.base = ack;
            let sends = self.pump(total);
            return (sends, self.base >= total);
        }
        if self.next > self.base && self.retx_base != Some(self.base) {
            self.retx_base = Some(self.base);
            self.next = self.base;
            return (self.pump(total), false);
        }
        (Vec::new(), false)
    }

    /// ACK 超时兜底：整窗重发 [base, next)。整窗全丢时不会有重复 ACK，
    /// 只能靠超时驱动；同时标记停滞点，避免随后的重复 ACK 再触发一次重传。
    pub fn go_back_n(&mut self, total: usize) -> Vec<usize> {
        if self.next <= self.base {
            return Vec::new();
        }
        self.retx_base = Some(self.base);
        self.next = self.base;
        self.pump(total)
    }
}

/// 接收方乱序缓存 + 连续前沿（按序消费）
#[derive(Debug, Default)]
pub struct RecvFrontier {
    /// 下一个仍缺失的连续序号（= 回给发送方的累计 ACK 值）
    next: usize,
    /// 前沿之后的乱序帧缓存：gseq → 原始消息
    buf: HashMap<usize, Value>,
}

impl RecvFrontier {
    pub fn new() -> Self {
        Self::default()
    }

    /// 落入一帧：重复帧（已缓存或已消费）只回当前前沿；新帧缓存后
    /// 从前沿起按序取出全部连续帧。
    /// 返回 (按序就绪待派发的消息, 当前前沿, 是否重复帧)
    pub fn insert(&mut self, gseq: usize, msg: Value) -> (Vec<Value>, usize, bool) {
        if gseq < self.next {
            return (Vec::new(), self.next, true);
        }
        if self.buf.contains_key(&gseq) {
            return (Vec::new(), self.next, true);
        }
        self.buf.insert(gseq, msg);
        let mut ready = Vec::new();
        while let Some(m) = self.buf.remove(&self.next) {
            ready.push(m);
            self.next += 1;
        }
        (ready, self.next, false)
    }
}
