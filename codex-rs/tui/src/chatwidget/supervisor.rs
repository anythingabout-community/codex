//! Supervisor task state; execution uses the normal child conversation view.

use super::*;
use codex_app_server_protocol::ThreadSupervisorUpdatedNotification;

#[derive(Default)]
pub(super) struct SupervisorUi {
    pub state: Option<codex_protocol::supervisor::SupervisorState>,
    implementer: bool,
}

impl ChatWidget {
    pub(crate) fn set_supervisor_conversation_role(&mut self, role: Option<&str>) {
        self.supervisor.implementer = role == Some("implementer");
        if self.supervisor.implementer {
            self.set_parent_owned_thread();
            self.bottom_pane.set_placeholder_text(
                "Viewing Implementer · read only · Shift+Tab to switch conversations".into(),
            );
        }
    }

    pub(crate) fn supervisor_enabled(&self) -> bool {
        self.config.features.enabled(Feature::ContinuousPlanning) && !self.supervisor.implementer
    }

    pub(super) fn goals_enabled(&self) -> bool {
        self.config.features.enabled(Feature::Goals)
            && !self.config.features.enabled(Feature::ContinuousPlanning)
    }

    pub(super) fn on_supervisor_updated(&mut self, update: ThreadSupervisorUpdatedNotification) {
        if !self.supervisor_enabled() {
            return;
        }
        if self
            .supervisor
            .state
            .as_ref()
            .is_none_or(|state| state.revision <= update.state.revision)
        {
            self.supervisor.state = Some(update.state);
        }
        if let Some(plan) = update.plan {
            self.bottom_pane
                .update_timed_plan(plan, update.thread_id, update.observed_at);
        }
        self.request_redraw();
    }
}
