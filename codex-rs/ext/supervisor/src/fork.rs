use crate::runtime::PlanRuntime;
use anyhow::Result;
use codex_core::CodexThread;
use codex_protocol::ThreadId;

/// Copies persisted Supervisor state and its owned Implementer into a fork.
///
/// The destination is paused until the user explicitly resumes it. The state database is the
/// source of truth, so this works whether the source Supervisor and Implementer are resident or
/// cold.
pub async fn fork_supervisor(source_thread_id: ThreadId, destination: &CodexThread) -> Result<()> {
    let Some(destination_runtime) = destination.thread_extension_data().get::<PlanRuntime>() else {
        return Ok(());
    };
    let plan = destination_runtime
        .db
        .read_thread_plan(source_thread_id)
        .await?;
    let Some(state) = destination_runtime
        .db
        .read_supervisor(source_thread_id)
        .await?
    else {
        anyhow::ensure!(
            plan.is_none(),
            "Supervisor plan has no persisted ownership state for {source_thread_id}"
        );
        return Ok(());
    };
    let source_implementer_id = plan.as_ref().and_then(|_| {
        state
            .implementer_thread_id
            .as_deref()
            .map(ThreadId::from_string)
    });
    let source_implementer_id = source_implementer_id.transpose()?;
    destination_runtime
        .adopt_fork_snapshot(plan, state, source_implementer_id)
        .await
}
