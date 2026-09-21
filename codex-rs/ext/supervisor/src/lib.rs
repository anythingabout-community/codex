//! User-facing supervision with a single, host-owned Implementer.
#![recursion_limit = "256"]
mod actions;
mod control;
mod evidence;
mod extension;
mod instructions;
mod message_items;
mod message_parser;
mod messages;
mod model;
mod runtime;
mod tool;

pub use extension::install;
pub use runtime::ImplementerBinding;
pub use runtime::PlanUpdateSink;
pub use runtime::SupervisorUpdateSink;

/// Pauses the whole task even while the Supervisor conversation itself is idle.
pub async fn suspend(thread: &codex_core::CodexThread) -> anyhow::Result<()> {
    if let Some(runtime) = thread.thread_extension_data().get::<runtime::PlanRuntime>() {
        runtime.suspend().await?;
    }
    Ok(())
}

pub fn is_supervisor(thread: &codex_core::CodexThread) -> bool {
    thread
        .thread_extension_data()
        .get::<runtime::PlanRuntime>()
        .is_some()
}

impl ImplementerBinding {
    pub fn supervisor_thread_id(&self) -> Option<codex_protocol::ThreadId> {
        self.runtime.upgrade().map(|r| r.thread_id)
    }

    pub async fn record_activity(&self, activity: &str) -> anyhow::Result<i64> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("Supervisor stopped"))?;
        anyhow::ensure!(
            runtime.implementer_matches(&self.execution_id).await,
            "stale Implementer activity"
        );
        let sequence = runtime
            .db
            .append_supervisor_activity(runtime.thread_id, activity)
            .await?;
        runtime.state.lock().await.activity_sequence = sequence;
        Ok(sequence)
    }

    pub async fn presentation(
        &self,
        turn_id: &str,
    ) -> Option<(codex_protocol::ThreadId, String, String, String)> {
        let runtime = self.runtime.upgrade()?;
        if !runtime.implementer_matches(&self.execution_id).await {
            return None;
        }
        let turns = self.turn_steps.lock().await;
        let (_, step_id, title) = turns.iter().find(|(id, _, _)| id == turn_id)?;
        Some((
            runtime.thread_id,
            self.execution_id.clone(),
            step_id.clone(),
            title.clone(),
        ))
    }
}
