//! Braille spinner element — `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` 80ms 切帧。
//!
//! 替换 inline_action loading 卡的静态 Circle icon,视觉等价于 opencode TUI
//! `<spinner frames=...>` 元件。
//!
//! 用法:
//! ```ignore
//! let spinner = BrailleSpinner::new(
//!     family_id,
//!     font_size,
//!     color,
//!     spinner_state_handle.clone(),
//! );
//! ```
//! `SpinnerStateHandle` 必须存在 view struct 跨 render 持久化(否则 Instant 每帧
//! 重置 → 永远停在第 0 帧)。同 ShimmeringTextStateHandle 模式。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use instant::Instant;
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::Vector2F;
use warpui::elements::{Element, Point, Text};
use warpui::event::DispatchedEvent;
use warpui::fonts::FamilyId;
use warpui::{
    AfterLayoutContext, AppContext, EventContext, LayoutContext, PaintContext, SizeConstraint,
};

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_INTERVAL_MS: u64 = 80;

#[derive(Clone)]
pub struct SpinnerStateHandle(Arc<Mutex<Instant>>);

impl Default for SpinnerStateHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl SpinnerStateHandle {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }

    fn frame_idx(&self) -> usize {
        let start = *self.0.lock().expect("spinner state poisoned");
        let elapsed_ms = start.elapsed().as_millis() as u64;
        ((elapsed_ms / FRAME_INTERVAL_MS) as usize) % FRAMES.len()
    }
}

pub struct BrailleSpinner {
    state: SpinnerStateHandle,
    color: ColorU,
    family_id: FamilyId,
    font_size: f32,
    inner: Option<Text>,
    size: Option<Vector2F>,
    origin: Option<Point>,
}

impl BrailleSpinner {
    pub fn new(
        family_id: FamilyId,
        font_size: f32,
        color: impl Into<ColorU>,
        state: SpinnerStateHandle,
    ) -> Self {
        Self {
            state,
            color: color.into(),
            family_id,
            font_size,
            inner: None,
            size: None,
            origin: None,
        }
    }
}

impl Element for BrailleSpinner {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        let frame = FRAMES[self.state.frame_idx()];
        // braille 字符是等宽,但仍每帧 layout 一次以确保字体/字号变更立即生效。
        // 单字符 layout 成本可忽略。
        let mut text =
            Text::new_inline(frame, self.family_id, self.font_size).with_color(self.color);
        let size = text.layout(constraint, ctx, app);
        self.inner = Some(text);
        self.size = Some(size);
        size
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        if let Some(t) = self.inner.as_mut() {
            t.after_layout(ctx, app);
        }
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.origin = Some(Point::from_vec2f(origin, ctx.scene.z_index()));
        if let Some(t) = self.inner.as_mut() {
            t.paint(origin, ctx, app);
        }
        // 关键:每帧 paint 完请求 80ms 后再次重绘,触发下一帧字符切换。
        // 不调用 repaint_after 则 spinner 静止——这是动画的引擎心跳。
        ctx.repaint_after(Duration::from_millis(FRAME_INTERVAL_MS));
    }

    fn size(&self) -> Option<Vector2F> {
        self.size
    }

    fn origin(&self) -> Option<Point> {
        self.origin
    }

    fn dispatch_event(
        &mut self,
        _: &DispatchedEvent,
        _: &mut EventContext,
        _: &AppContext,
    ) -> bool {
        false
    }
}
