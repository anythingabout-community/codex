//! Implementer reports are checked against host-recorded execution history.

use crate::runtime::PlanRuntime;
use crate::runtime::now;
use anyhow::Result;
use anyhow::ensure;
use codex_history::RolloutItem;
use codex_protocol::continuous_planning::*;
use codex_protocol::models::ResponseItem;

impl PlanRuntime {
    async fn implementer_id(&self) -> Option<codex_protocol::ThreadId> {
        self.state
            .lock()
            .await
            .implementer_thread_id
            .as_deref()
            .and_then(|id| codex_protocol::ThreadId::from_string(id).ok())
    }

    async fn implementer_history(&self) -> Result<Vec<ResponseItem>> {
        let implementer_id = self
            .implementer_id()
            .await
            .ok_or_else(|| anyhow::anyhow!("no Implementer execution exists"))?;
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("host stopped"))?;
        Ok(manager
            .read_thread_history(implementer_id)
            .await
            .map_err(|error| anyhow::anyhow!("failed to read Implementer history: {error}"))?
            .into_iter()
            .filter_map(|item| match item {
                RolloutItem::ResponseItem(envelope) => Some(envelope.item),
                _ => None,
            })
            .collect())
    }
}

impl PlanRuntime {
    /// Searches the private Implementer history and returns bounded matching records.
    pub async fn inspect(&self, query: Option<&str>, offset: usize) -> Result<String> {
        ensure!(offset <= 64 * 1024, "history offset is too large");
        ensure!(
            self.implementer_id().await.is_some(),
            "no Implementer execution exists"
        );
        let needle = query.map(str::to_lowercase);
        let history = self.implementer_history().await?;
        let mut output = String::new();
        let mut matched = 0usize;
        for item in history.into_iter().rev() {
            let mut value = serde_json::to_value(item)?;
            if let Some(object) = value.as_object_mut() {
                object.remove("internal_chat_message_metadata_passthrough");
            }
            let text = value.to_string();
            if needle
                .as_ref()
                .is_some_and(|needle| !text.to_lowercase().contains(needle))
            {
                continue;
            }
            if matched < offset {
                matched += 1;
                continue;
            }
            if output.len().saturating_add(text.len() + 1) > 8192 {
                break;
            }
            output.push_str(&text);
            output.push('\n');
            matched += 1;
            if matched.saturating_sub(offset) == 32 {
                break;
            }
        }
        ensure!(!output.is_empty(), "no matching Implementer history");
        Ok(output)
    }

    pub async fn evidence(&self) -> Result<String> {
        let version = self
            .plan
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no task"))?
            .version;
        let state = self.state.lock().await.clone();
        if self.implementer_id().await.is_none() {
            let records = self
                .db
                .list_supervisor_activity(
                    self.thread_id,
                    state.activity_sequence.saturating_sub(16),
                    16,
                )
                .await?;
            let output = records
                .into_iter()
                .map(|(_, value)| value)
                .collect::<Vec<_>>()
                .join("\n");
            ensure!(
                !output.is_empty(),
                "no saved execution evidence; ask Implementer to verify the current state"
            );
            *self.evidence_review.lock().await = Some((version, output.clone()));
            return Ok(output);
        }
        let items = self.implementer_history().await?;
        let control_calls: std::collections::HashSet<_> = items
            .iter()
            .filter_map(|item| match item {
                ResponseItem::FunctionCall { name, call_id, .. }
                    if name == "continuous_planning" =>
                {
                    Some(call_id.as_str())
                }
                _ => None,
            })
            .collect();
        let recorded: Vec<_> = items
            .iter()
            .rev()
            .filter(|item| match item {
                ResponseItem::FunctionCallOutput { call_id, .. } => call_id
                    .as_deref()
                    .is_none_or(|id| !control_calls.contains(id)),
                ResponseItem::CustomToolCallOutput { .. }
                | ResponseItem::WebSearchCall { .. }
                | ResponseItem::ImageGenerationCall { .. } => true,
                // Hosted search evidence can carry source annotations on Implementer's final
                // message. Keep that report private, but available for Supervisor to inspect.
                ResponseItem::Message { role, .. } => role == "assistant",
                _ => false,
            })
            .take(16)
            .collect();
        let evidence = if recorded.is_empty() {
            items.iter().rev().take(8).collect()
        } else {
            recorded
        };
        let mut output = String::new();
        for item in evidence {
            let mut value = serde_json::to_value(item)?;
            if let Some(object) = value.as_object_mut() {
                object.remove("internal_chat_message_metadata_passthrough");
            }
            let value = value.to_string();
            output.push_str(&value);
            output.push('\n');
        }
        *self.evidence_review.lock().await = Some((version, output.clone()));
        Ok(output)
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "acceptance commits one verified revision"
    )]
    pub async fn accept(&self, version: i64, step_id: &str, evidence: Vec<String>) -> Result<()> {
        ensure!(
            !evidence.is_empty()
                && evidence.len() <= 8
                && evidence
                    .iter()
                    .all(|item| !item.trim().is_empty() && item.len() <= 1024),
            "record bounded verification evidence"
        );
        let _control = self.control.lock().await;
        ensure!(
            self.evidence_review
                .lock()
                .await
                .as_ref()
                .is_some_and(|(reviewed, _)| *reviewed == version),
            "read the submitted execution evidence before acceptance"
        );
        if let Some(implementer) = self.implementer.lock().await.as_ref() {
            ensure!(
                implementer.thread.agent_status().await
                    != codex_protocol::protocol::AgentStatus::Running,
                "wait for Implementer to stop before acceptance"
            );
        }
        let mut current = self.plan.lock().await;
        let mut plan = current.clone().ok_or_else(|| anyhow::anyhow!("no task"))?;
        ensure!(plan.version == version, "stale version; read current task");
        let step = plan
            .steps
            .iter_mut()
            .find(|step| step.definition.id == step_id)
            .ok_or_else(|| anyhow::anyhow!("unknown step"))?;
        ensure!(
            step.state == PlanStepState::Reviewing && !step.evidence.is_empty(),
            "Implementer must finish with a final reply before acceptance"
        );
        step.state = PlanStepState::Completed;
        step.completed_at = Some(now());
        plan.review = PlanReviewState::Idle;
        plan.reason = evidence.join("\n");
        crate::model::finish_revision(&mut plan, now());
        self.db.append_thread_plan(self.thread_id, &plan).await?;
        *current = Some(plan.clone());
        (self.sink)(self.thread_id, plan);
        let saved = current
            .clone()
            .ok_or_else(|| anyhow::anyhow!("plan was lost while accepting the step"))?;
        drop(current);
        self.write_plan_file(&saved).await
    }
}
