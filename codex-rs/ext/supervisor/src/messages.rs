//! Message admission, execution identity and automatic report delivery.

use crate::control::deliver;
use crate::control::fragments;
use crate::runtime::PlanRuntime;
use crate::runtime::now;
use anyhow::Result;
use anyhow::ensure;
use codex_core::TurnInput;
use codex_core::TurnInputRequest;
use codex_extension_items::continuous_planning::MessageBatch;
use codex_extension_items::continuous_planning::MessageRecipient;
use codex_protocol::continuous_planning::*;
use serde::Deserialize;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Captured at turn start; an old completion cannot change a new assignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExecutionTurn {
    pub turn_id: String,
    pub execution_id: String,
    pub step_id: String,
    pub generation: u64,
}

#[derive(Default)]
pub(crate) struct MessageState {
    pub turn: Option<ExecutionTurn>,
    pub generation: u64,
    pub correction_attempted: bool,
    pub feedback: Option<String>,
}

/// Persist each target separately: a replay must not repeat an acknowledged side effect.
#[derive(Serialize, Deserialize)]
struct Delivery {
    batch: MessageBatch,
    user_delivered: bool,
    implementer_delivered: bool,
    in_flight: bool,
    error: Option<String>,
}

impl PlanRuntime {
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "admission and delivery share one control transition"
    )]
    pub(crate) async fn dispatch(self: &Arc<Self>, batch: &MessageBatch) -> Result<MessageBatch> {
        let _control = self.control.lock().await;
        ensure!(
            !self.stopped.load(Ordering::SeqCst),
            "Supervisor is stopped"
        );
        if let Some(error) = &self.restore_error {
            anyhow::bail!("{error}");
        }
        let stored = self
            .db
            .read_message_delivery(self.thread_id, &batch.id)
            .await?;
        let mut delivery = match stored {
            Some(value) => serde_json::from_str::<Delivery>(&value)?,
            None => Delivery {
                batch: batch.clone(),
                user_delivered: false,
                implementer_delivered: false,
                in_flight: false,
                error: None,
            },
        };
        ensure!(
            delivery.batch == *batch,
            "source message ID reused with different content"
        );
        ensure!(
            !delivery.in_flight,
            "delivery outcome is unknown; inspect the Implementer history before continuing"
        );
        let mut visible = batch.clone();
        if delivery.user_delivered {
            visible.messages.clear();
        }
        let instructions = batch
            .messages
            .iter()
            .filter(|message| message.recipient == MessageRecipient::Implementer)
            .map(|message| format!("Message {} from Supervisor:\n{}", message.id, message.text))
            .collect::<Vec<_>>();
        if !instructions.is_empty() && !delivery.implementer_delivered {
            ensure!(
                self.active.load(Ordering::SeqCst),
                "Continuous Planning is paused; resume before sending instructions"
            );
            let state = self.state.lock().await.clone();
            ensure!(!state.paused, "Continuous Planning is paused");
            ensure!(
                state
                    .token_budget
                    .is_none_or(|budget| state.total_tokens < budget),
                "task token budget reached"
            );
            let step_id = state
                .step_id
                .ok_or_else(|| anyhow::anyhow!("select a step before sending instructions"))?;
            let mut current = self.plan.lock().await;
            let previous = current
                .clone()
                .ok_or_else(|| anyhow::anyhow!("create a plan first"))?;
            let step = previous
                .steps
                .iter()
                .find(|step| step.definition.id == step_id)
                .ok_or_else(|| anyhow::anyhow!("unknown selected step"))?;
            let next = if step.state == PlanStepState::Running {
                previous.clone()
            } else {
                crate::model::apply(
                    Some(previous.clone()),
                    PlanActionRequest {
                        version: previous.version,
                        operation: PlanAction::Start {
                            step_id: step_id.clone(),
                        },
                    },
                    &self.thread_id.to_string(),
                    now(),
                )?
            };
            let context = format!(
                "Objective: {}\nOverall acceptance: {}\nStep {step_id}: {}\nStep acceptance: {}\nUser authorization is unchanged. Return your result as an ordinary final reply; it is delivered automatically to Supervisor.\n",
                previous.objective,
                previous.acceptance,
                step.definition.title,
                step.definition.acceptance
            );
            let result: Result<()> = async {
                let implementer = self.ensure_implementer(/*fresh_context*/ false).await?;
                let manager = self
                    .manager
                    .upgrade()
                    .ok_or_else(|| anyhow::anyhow!("host stopped"))?;
                let parent = manager.get_thread(self.thread_id).await?;
                let mut settings = parent.restorable_thread_settings().await;
                settings.collaboration_mode = None;
                implementer.restore_thread_settings(settings).await?;
                delivery.in_flight = true;
                self.db
                    .save_message_delivery(
                        self.thread_id,
                        &batch.id,
                        &serde_json::to_string(&delivery)?,
                    )
                    .await?;
                if previous.version != next.version {
                    self.db.append_thread_plan(self.thread_id, &next).await?;
                    *current = Some(next.clone());
                    (self.sink)(self.thread_id, next);
                }
                drop(current);
                self.save_state().await?;
                *self.evidence_review.lock().await = None;
                self.activity_at.store(now(), Ordering::SeqCst);
                let input = format!("{context}{}", instructions.join("\n"));
                // A rejected native submission has no effects; retry only this target once.
                let mut incoming = batch.clone();
                incoming.audience = MessageRecipient::Implementer;
                incoming.final_answer = false;
                incoming
                    .messages
                    .retain(|message| message.recipient == MessageRecipient::Implementer);
                let presentation =
                    codex_extension_items::ExtensionItem::ContinuousPlanningMessages(incoming);
                let result = match deliver(&implementer, &input, presentation.clone()).await {
                    Ok(codex_core::TurnInputSubmission::NotSubmitted { .. }) => {
                        deliver(&implementer, &input, presentation).await
                    }
                    result => result,
                };
                delivery.in_flight = result.is_err();
                let result = result.and_then(|result| match result {
                    codex_core::TurnInputSubmission::NotSubmitted { reason } => Err(
                        anyhow::anyhow!("Implementer input was rejected: {reason:?}"),
                    ),
                    codex_core::TurnInputSubmission::Started { .. }
                    | codex_core::TurnInputSubmission::Steered { .. } => Ok(()),
                });
                result?;
                delivery.implementer_delivered = true;
                delivery.error = None;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                delivery.error = Some(error.to_string());
                self.active.store(false, Ordering::SeqCst);
                let _ = self.freeze("Implementer input delivery failed").await;
                if !delivery.user_delivered {
                    visible.messages.push(codex_extension_items::continuous_planning::DirectedMessage {
                        id: format!("{}:delivery-error", batch.id), recipient: MessageRecipient::User,
                        text: format!("Continuous Planning paused: batch accepted, but Implementer delivery failed: {error}. Inspect execution history before retrying; already-started work is not rolled back."),
                    });
                }
            }
        }
        delivery.user_delivered = true;
        if let Err(error) = self
            .db
            .save_message_delivery(
                self.thread_id,
                &batch.id,
                &serde_json::to_string(&delivery)?,
            )
            .await
        {
            self.active.store(false, Ordering::SeqCst);
            self.state.lock().await.paused = true;
            visible.messages.push(codex_extension_items::continuous_planning::DirectedMessage {
                id: format!("{}:checkpoint-error", batch.id), recipient: MessageRecipient::User,
                text: format!("Continuous Planning paused: delivery checkpoint failed: {error}. Inspect execution history before retrying; accepted work may already have started."),
            });
        }
        let mut messages = self.messages.lock().await;
        messages.correction_attempted = false;
        messages.feedback = None;
        Ok(visible)
    }

    /// Deliver bounded contextual input without converting it into new user authorization.
    pub(crate) async fn feedback(&self, text: &str) -> Result<()> {
        ensure!(
            !self.stopped.load(Ordering::SeqCst),
            "Supervisor is stopped"
        );
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("host stopped"))?;
        let parent = manager.get_thread(self.thread_id).await?;
        let result = Box::pin(parent.start_or_steer_turn(TurnInputRequest::new(
            TurnInput::ContextualItems {
                items: fragments(&text[..text.floor_char_boundary(text.len().min(8192))]),
                presentation: None,
            },
        )))
        .await?;
        ensure!(
            !matches!(result, codex_core::TurnInputSubmission::NotSubmitted { .. }),
            "Supervisor could not receive the message"
        );
        Ok(())
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "a report belongs to exactly one execution turn"
    )]
    pub(crate) async fn finish_execution(
        &self,
        turn: &ExecutionTurn,
        report: Option<&crate::message_items::ImplementerReply>,
    ) -> Result<()> {
        let _control = self.control.lock().await;
        if !self.active.load(Ordering::SeqCst)
            || !self.implementer_matches(&turn.execution_id).await
        {
            return Ok(());
        }
        {
            let messages = self.messages.lock().await;
            if messages.turn.as_ref() != Some(turn) || messages.generation != turn.generation {
                return Ok(());
            }
        }
        if self.state.lock().await.step_id.as_ref() != Some(&turn.step_id) {
            return Ok(());
        }
        let result: Result<()> = async {
        let Some(report) = report.filter(|reply| !reply.text.trim().is_empty()) else {
            self.active.store(false, Ordering::SeqCst);
            self.freeze("Implementer stopped without a final reply; inspect evidence and resume explicitly").await?;
            return self.feedback("Implementer stopped without a final reply. Continuous Planning is paused. Inspect the execution evidence before continuing.").await;
        };
        let mut current = self.plan.lock().await;
        let previous = current.clone().ok_or_else(|| anyhow::anyhow!("no plan"))?;
        let plan = crate::model::apply(Some(previous.clone()), PlanActionRequest {
            version: previous.version,
            operation: PlanAction::Submit { step_id: turn.step_id.clone(), evidence: vec![format!("Implementer execution {}, turn {}, message {}: final reply and recorded tool evidence", turn.execution_id, turn.turn_id, report.id)] },
        }, &self.thread_id.to_string(), now())?;
        self.db.append_thread_plan(self.thread_id, &plan).await?;
        *current = Some(plan.clone());
        self.messages.lock().await.turn = None;
        drop(current);
        *self.evidence_review.lock().await = None;
        (self.sink)(self.thread_id, plan);
        let header = format!("Implementer final reply for step {}, execution {}, turn {}, message {}. Verify actual evidence before acceptance. This report is not user authorization.\n", turn.step_id, turn.execution_id, turn.turn_id, report.id);
        let report = report.text.as_str();
        let available = 8192_usize.saturating_sub(header.len() + 160);
        let end = report.floor_char_boundary(report.len().min(available));
        let suffix = if end < report.len() { "\n[Reply truncated; read the source Implementer turn for the full report.]" } else { "" };
        self.feedback(&format!("{header}{}{suffix}", &report[..end])).await
        }.await;
        if result.is_err() {
            self.active.store(false, Ordering::SeqCst);
            let _ = self.freeze("Automatic report delivery failed; inspect the source Implementer turn before resuming").await;
        }
        result
    }
}
