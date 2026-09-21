use super::StateRuntime;
use codex_protocol::ThreadId;
use codex_protocol::supervisor::SupervisorState;

impl StateRuntime {
    /// Loads admission and delivery outcomes without replaying any external effects.
    pub async fn read_message_delivery(
        &self,
        thread_id: ThreadId,
        source_id: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT snapshot FROM continuous_planning_messages WHERE thread_id = ? AND source_id = ?")
            .bind(thread_id.to_string()).bind(source_id).fetch_optional(self.pool.as_ref()).await?)
    }

    /// Checkpoints one bounded batch; callers serialize target delivery with admission.
    pub async fn save_message_delivery(
        &self,
        thread_id: ThreadId,
        source_id: &str,
        snapshot: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            snapshot.len() <= 65536,
            "message delivery snapshot exceeds limit"
        );
        sqlx::query("INSERT INTO continuous_planning_messages (thread_id, source_id, snapshot) VALUES (?, ?, ?) ON CONFLICT(thread_id, source_id) DO UPDATE SET snapshot = excluded.snapshot")
            .bind(thread_id.to_string()).bind(source_id).bind(snapshot).execute(self.pool.as_ref()).await?;
        Ok(())
    }

    pub async fn read_supervisor(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<SupervisorState>> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT snapshot FROM thread_supervisors WHERE thread_id = ?")
                .bind(thread_id.to_string())
                .fetch_optional(self.pool.as_ref())
                .await?;
        let Some(value) = value else {
            return Ok(None);
        };
        let mut state: SupervisorState = serde_json::from_str(&value)?;
        state.activity_sequence = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence), 0) FROM thread_supervisor_activity WHERE thread_id = ?",
        )
        .bind(thread_id.to_string())
        .fetch_one(self.pool.as_ref())
        .await?;
        Ok(Some(state))
    }

    pub async fn save_supervisor(
        &self,
        thread_id: ThreadId,
        state: &SupervisorState,
    ) -> anyhow::Result<()> {
        sqlx::query("INSERT INTO thread_supervisors (thread_id, snapshot) VALUES (?, ?) ON CONFLICT(thread_id) DO UPDATE SET snapshot = excluded.snapshot")
            .bind(thread_id.to_string()).bind(serde_json::to_string(state)?).execute(self.pool.as_ref()).await?;
        Ok(())
    }

    /// Appends the presentation event and assigns its replay cursor atomically.
    pub async fn append_supervisor_activity(
        &self,
        thread_id: ThreadId,
        activity: &str,
    ) -> anyhow::Result<i64> {
        anyhow::ensure!(
            activity.len() <= 1024 * 1024,
            "supervisor activity exceeds storage limit"
        );
        let sequence: i64 = sqlx::query_scalar("INSERT INTO thread_supervisor_activity (thread_id, sequence, activity) SELECT ?, COALESCE(MAX(sequence), 0) + 1, ? FROM thread_supervisor_activity WHERE thread_id = ? RETURNING sequence")
            .bind(thread_id.to_string()).bind(activity).bind(thread_id.to_string()).fetch_one(self.pool.as_ref()).await?;
        Ok(sequence)
    }

    pub async fn list_supervisor_activity(
        &self,
        thread_id: ThreadId,
        after: i64,
        limit: u32,
    ) -> anyhow::Result<Vec<(i64, String)>> {
        Ok(sqlx::query_as("SELECT sequence, activity FROM thread_supervisor_activity WHERE thread_id = ? AND sequence > ? ORDER BY sequence LIMIT ?")
            .bind(thread_id.to_string()).bind(after).bind(limit.clamp(1, 100)).fetch_all(self.pool.as_ref()).await?)
    }
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
