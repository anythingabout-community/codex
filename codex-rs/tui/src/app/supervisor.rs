//! Discover supervision pairs through the existing conversation navigator.

use super::*;

impl App {
    pub(super) fn cache_supervisor_conversations(&mut self, notification: &ServerNotification) {
        let ServerNotification::ThreadSupervisorUpdated(update) = notification else {
            return;
        };
        let Ok(supervisor) = ThreadId::from_string(&update.thread_id) else {
            return;
        };
        if self.agent_navigation.get(&supervisor).is_none() {
            self.upsert_agent_picker_thread(
                supervisor,
                /*agent_nickname*/ None,
                Some("supervisor".into()),
                /*is_closed*/ false,
            );
        }
        if let Some(implementer) = update
            .state
            .implementer_thread_id
            .as_deref()
            .and_then(|id| ThreadId::from_string(id).ok())
            && self.agent_navigation.get(&implementer).is_none()
        {
            self.upsert_agent_picker_thread(
                implementer,
                /*agent_nickname*/ None,
                Some("implementer".into()),
                /*is_closed*/ false,
            );
        }
    }
}
