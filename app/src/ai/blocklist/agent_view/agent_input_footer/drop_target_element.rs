//! Phase 4c-3 task 5.
//!
//! `AttachmentDropTargetElement` is a thin wrapper around the standard
//! agent-input-footer render output that intercepts drag-and-drop file events
//! and dispatches `AgentInputFooterAction::DragAndDropFiles` when the cursor
//! is within the element bounds.  Mirrors the pattern in
//! `terminal_size_element.rs` exactly.

use std::any::Any;

use warpui::{
    elements::Point,
    event::DispatchedEvent,
    geometry::{rect::RectF, vector::Vector2F},
    AfterLayoutContext, AppContext, Element, Event, EventContext, LayoutContext, PaintContext,
    SizeConstraint,
};

use super::AgentInputFooterAction;

pub struct AttachmentDropTargetElement {
    child: Box<dyn Element>,
}

impl AttachmentDropTargetElement {
    pub fn new(child: Box<dyn Element>) -> Self {
        AttachmentDropTargetElement { child }
    }

    fn mouse_position_is_in_bounds(&self, position: Vector2F) -> bool {
        let Some(bounds) = self.bounds() else {
            return false;
        };
        bounds.contains_point(position)
    }
}

impl Element for AttachmentDropTargetElement {
    fn layout(
        &mut self,
        constraint: SizeConstraint,
        ctx: &mut LayoutContext,
        app: &AppContext,
    ) -> Vector2F {
        self.child.layout(constraint, ctx, app)
    }

    fn after_layout(&mut self, ctx: &mut AfterLayoutContext, app: &AppContext) {
        self.child.after_layout(ctx, app);
    }

    fn paint(&mut self, origin: Vector2F, ctx: &mut PaintContext, app: &AppContext) {
        self.child.paint(origin, ctx, app)
    }

    fn size(&self) -> Option<Vector2F> {
        self.child.size()
    }

    fn origin(&self) -> Option<Point> {
        self.child.origin()
    }

    fn bounds(&self) -> Option<RectF> {
        self.child.bounds()
    }

    fn parent_data(&self) -> Option<&dyn Any> {
        self.child.parent_data()
    }

    fn dispatch_event(
        &mut self,
        event: &DispatchedEvent,
        ctx: &mut EventContext,
        app: &AppContext,
    ) -> bool {
        let handled_by_child = self.child.dispatch_event(event, ctx, app);
        let Some(z_index) = self.z_index() else {
            return handled_by_child;
        };

        if !handled_by_child {
            if let Some(event_at_z_index) = event.at_z_index(z_index, ctx) {
                match event_at_z_index {
                    Event::DragFiles { .. } | Event::DragFileExit => {
                        // Drag-over highlighting is skipped (plan permits this).
                        return true;
                    }
                    Event::DragAndDropFiles { paths, location } => {
                        if self.mouse_position_is_in_bounds(*location) && !paths.is_empty() {
                            let paths_owned: Vec<std::path::PathBuf> =
                                paths.iter().map(std::path::PathBuf::from).collect();
                            ctx.dispatch_typed_action(
                                AgentInputFooterAction::DragAndDropFiles(paths_owned),
                            );
                        }
                        return true;
                    }
                    _ => {}
                }
            }
        }
        handled_by_child
    }
}

#[cfg(test)]
#[path = "drop_target_element_tests.rs"]
mod tests;
