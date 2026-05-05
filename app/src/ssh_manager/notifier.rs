//! 全局 SSH 树变更广播 — 任何 view 改了树结构(增/删/改名/改 server 字段)
//! 后调一次 `notify`,SshManagerPanel 等订阅者据此 refresh。
//!
//! 跟 `KeybindingChangedNotifier`(`app/src/settings_view/keybindings.rs:72`)
//! 一个套路:Empty struct + SingletonEntity + 单个 Event 变体。

use warpui::{Entity, SingletonEntity};

#[derive(Default)]
pub struct SshTreeChangedNotifier {}

impl SshTreeChangedNotifier {
    pub fn new() -> Self {
        Default::default()
    }
}

#[derive(Clone, Debug)]
pub enum SshTreeChangedEvent {
    /// 节点列表 / server 详情已变,需要重新 list_nodes。
    TreeChanged,
}

impl Entity for SshTreeChangedNotifier {
    type Event = SshTreeChangedEvent;
}

impl SingletonEntity for SshTreeChangedNotifier {}
