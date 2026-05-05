use std::path::PathBuf;

use warpui::{ViewContext, ViewHandle};

use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::attachment_utils::attachments_download_dir;
use crate::pane_group::PaneGroup;
use crate::terminal::TerminalView;

#[derive(Clone, Debug)]
pub(crate) struct HiddenChildAgentTaskContext {
    pub task_id: AmbientAgentTaskId,
    pub working_dir: Option<PathBuf>,
}

pub(crate) fn apply_hidden_child_agent_task_context(
    terminal_view: &ViewHandle<TerminalView>,
    task_context: &HiddenChildAgentTaskContext,
    ctx: &mut ViewContext<PaneGroup>,
) {
    let task_id = task_context.task_id;
    let working_dir = task_context.working_dir.clone();

    terminal_view.update(ctx, move |terminal_view, ctx| {
        terminal_view
            .ai_controller()
            .update(ctx, |controller, ctx| {
                controller.set_ambient_agent_task_id(Some(task_id), ctx);
                if let Some(working_dir) = working_dir.as_deref() {
                    controller.set_attachments_download_dir(attachments_download_dir(working_dir));
                }
            });
    });
}
