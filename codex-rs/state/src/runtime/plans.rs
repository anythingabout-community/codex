use super::StateRuntime;
use codex_protocol::ThreadId;
use codex_protocol::continuous_planning::ContinuousPlanning;

impl StateRuntime {
    pub async fn read_thread_plan(
        &self,
        thread_id: ThreadId,
    ) -> anyhow::Result<Option<ContinuousPlanning>> {
        let snapshot: Option<(String, i64)> = sqlx::query_as(
            "SELECT snapshot, checkpoint_at FROM thread_plan_revisions WHERE thread_id = ? ORDER BY version DESC LIMIT 1",
        )
        .bind(thread_id.to_string())
        .fetch_optional(self.pool.as_ref())
        .await?;
        snapshot
            .map(|(value, checkpoint)| {
                let mut plan: ContinuousPlanning = serde_json::from_str(&value)?;
                plan.updated_at = plan.updated_at.max(checkpoint);
                Ok(plan)
            })
            .transpose()
    }

    /// Advances the crash-recovery clock without producing a logical plan revision.
    pub async fn checkpoint_thread_plan(
        &self,
        thread_id: ThreadId,
        version: i64,
        now: i64,
    ) -> anyhow::Result<()> {
        sqlx::query("UPDATE thread_plan_revisions SET checkpoint_at = MAX(checkpoint_at, ?) WHERE thread_id = ? AND version = ?")
            .bind(now).bind(thread_id.to_string()).bind(version).execute(self.pool.as_ref()).await?;
        Ok(())
    }

    /// Appends a revision only if it immediately follows the current version.
    pub async fn append_thread_plan(
        &self,
        thread_id: ThreadId,
        plan: &ContinuousPlanning,
    ) -> anyhow::Result<()> {
        let snapshot = serde_json::to_string(plan)?;
        anyhow::ensure!(snapshot.len() <= 128 * 1024, "plan exceeds storage limit");
        let result = sqlx::query(
            "INSERT INTO thread_plan_revisions (thread_id, version, snapshot, checkpoint_at)
             SELECT ?, ?, ?, ? WHERE ? = COALESCE(
                 (SELECT MAX(version) FROM thread_plan_revisions WHERE thread_id = ?), 0) + 1",
        )
        .bind(thread_id.to_string())
        .bind(plan.version)
        .bind(snapshot)
        .bind(plan.updated_at)
        .bind(plan.version)
        .bind(thread_id.to_string())
        .execute(self.pool.as_ref())
        .await?;
        anyhow::ensure!(
            result.rows_affected() == 1,
            "stale plan version; read the current plan before retrying"
        );
        Ok(())
    }

    pub async fn list_thread_plan_history(
        &self,
        thread_id: ThreadId,
        before: i64,
        limit: u32,
    ) -> anyhow::Result<Vec<ContinuousPlanning>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT snapshot FROM thread_plan_revisions WHERE thread_id = ? AND version < ? ORDER BY version DESC LIMIT ?",
        )
        .bind(thread_id.to_string())
        .bind(before)
        .bind(limit.clamp(1, 100))
        .fetch_all(self.pool.as_ref())
        .await?;
        rows.into_iter()
            .map(|value| serde_json::from_str(&value).map_err(Into::into))
            .collect()
    }
}

#[cfg(test)]
#[path = "plans_tests.rs"]
mod tests;
