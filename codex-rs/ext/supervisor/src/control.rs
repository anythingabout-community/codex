//! Serial ownership transitions for the one Implementer slot.

use crate::runtime::Implementer;
use crate::runtime::ImplementerBinding;
use crate::runtime::PlanRuntime;
use crate::runtime::now;
use anyhow::Result;
use anyhow::ensure;
use codex_core::CodexThread;
use codex_core::StartIfIdleSubmission;
use codex_core::StartThreadOptions;
use codex_core::TurnInput;
use codex_core::TurnInputRequest;
use codex_core::context::ContextualUserFragment;
use codex_core::context::ContinuousPlanningFragment;
use codex_extension_api::SupervisorSession;
use codex_features::Feature;
use codex_history::InitialHistory;
use codex_protocol::ThreadId;
use codex_protocol::continuous_planning::*;
use codex_protocol::protocol::AgentStatus;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::supervisor::SupervisorState;
use std::sync::Arc;
use std::sync::atomic::Ordering;

impl PlanRuntime {
    pub(crate) async fn adopt_fork_snapshot(
        self: &Arc<Self>,
        plan: Option<ContinuousPlanning>,
        mut state: SupervisorState,
        source_implementer: Option<ThreadId>,
    ) -> Result<()> {
        state.implementer_thread_id = None;
        state.execution_id = None;
        state.plan_path = None;
        state.pending_attention = None;
        state.paused = true;
        self.active.store(false, Ordering::SeqCst);
        *self.state.lock().await = state;
        if let Some(mut plan) = plan {
            plan.id = self.thread_id.to_string();
            plan.version = 1;
            let timestamp = now();
            for step in &mut plan.steps {
                if step.running_since.is_some() {
                    step.elapsed_seconds = step.elapsed_at(timestamp);
                }
                if matches!(
                    step.state,
                    PlanStepState::Running | PlanStepState::Reviewing
                ) {
                    step.running_since = None;
                    step.state = PlanStepState::Blocked;
                }
            }
            plan.updated_at = timestamp;
            plan.next_review_at = timestamp;
            if plan
                .steps
                .iter()
                .any(|step| step.state == PlanStepState::Blocked)
            {
                plan.review = PlanReviewState::Due;
            }
            plan.reason = "Forked from the source Supervisor; resume explicitly".into();
            self.db.append_thread_plan(self.thread_id, &plan).await?;
            *self.plan.lock().await = Some(plan.clone());
            (self.sink)(self.thread_id, plan.clone());
            self.write_plan_file(&plan).await?;
        }
        self.save_state().await?;
        if let Some(source_id) = source_implementer {
            self.fork_implementer_from(source_id).await?;
            self.state.lock().await.paused = true;
            self.active.store(false, Ordering::SeqCst);
            self.save_state().await?;
        }
        Ok(())
    }

    async fn fork_implementer_from(self: &Arc<Self>, source_id: ThreadId) -> Result<()> {
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("host stopped"))?;
        let parent = manager.get_thread(self.thread_id).await?;
        let mut config = parent.config().await.as_ref().clone();
        config.agent_max_depth = config.agent_max_depth.saturating_add(1);
        config.features.disable(Feature::Goals)?;
        config.update_plan_enabled = false;
        config.experimental_request_user_input_enabled = false;
        let execution_id = ThreadId::new().to_string();
        let source_config = parent.session_source();
        let agent_path = source_config
            .get_agent_path()
            .unwrap_or_else(codex_protocol::AgentPath::root)
            .join("implementer")
            .map_err(anyhow::Error::msg)?;
        let depth = match source_config {
            SessionSource::SubAgent(SubAgentSource::ThreadSpawn { depth, .. }) => depth + 1,
            _ => 1,
        };
        let mut options = StartThreadOptions::new(config);
        options.history_mode = Some(codex_protocol::protocol::ThreadHistoryMode::Paginated);
        options.session_source = Some(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: self.thread_id,
            depth,
            agent_path: Some(agent_path),
            agent_nickname: None,
            agent_role: Some("implementer".to_string()),
        }));
        options
            .thread_extension_init
            .insert(SupervisorSession::Implementer);
        options.thread_extension_init.insert(ImplementerBinding {
            runtime: Arc::downgrade(self),
            execution_id: execution_id.clone(),
            turn_steps: tokio::sync::Mutex::new(std::collections::VecDeque::new()),
        });
        let spawned = manager
            .spawn_subagent_with_parent(source_id, self.thread_id, options)
            .await?;
        {
            let mut state = self.state.lock().await;
            state.implementer_thread_id = Some(spawned.thread_id.to_string());
            state.execution_id = Some(execution_id.clone());
        }
        *self.implementer.lock().await = Some(Implementer {
            thread: spawned.thread,
            execution_id,
        });
        Ok(())
    }

    /// Select a validated step and immediately hand it to the Implementer.
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "selection serializes with dispatch"
    )]
    pub async fn select(self: &Arc<Self>, version: i64, step_id: String) -> Result<()> {
        let _control = self.control.lock().await;
        ensure!(
            !self.stopped.load(Ordering::SeqCst),
            "Supervisor is stopped"
        );
        if let Some(implementer) = self.implementer.lock().await.as_ref() {
            ensure!(
                implementer.thread.agent_status().await != AgentStatus::Running,
                "pause the Implementer before selecting another step"
            );
        }
        let next = crate::model::apply(
            self.plan.lock().await.clone(),
            PlanActionRequest {
                version,
                operation: PlanAction::Start {
                    step_id: step_id.clone(),
                },
            },
            &self.thread_id.to_string(),
            now(),
        )?;
        let should_start = {
            let mut state = self.state.lock().await;
            ensure!(
                state
                    .token_budget
                    .is_none_or(|budget| state.total_tokens < budget),
                "task token budget reached; ask the user before increasing it"
            );
            state.step_id = Some(step_id.clone());
            let should_start = !state.paused;
            self.active.store(should_start, Ordering::SeqCst);
            should_start
        };
        {
            let mut plan = self.plan.lock().await;
            if plan
                .as_ref()
                .is_none_or(|current| current.version != next.version)
            {
                self.db.append_thread_plan(self.thread_id, &next).await?;
                *plan = Some(next.clone());
                (self.sink)(self.thread_id, next.clone());
            }
        }
        *self.evidence_review.lock().await = None;
        self.save_state().await?;
        self.write_plan_file(&next).await?;
        if should_start {
            let result = self.start_selected_step(&step_id).await;
            if let Err(error) = &result {
                self.active.store(false, Ordering::SeqCst);
                let _ = self
                    .freeze("automatic Implementer kickoff failed; inspect and resume explicitly")
                    .await;
                return Err(anyhow::anyhow!("{error}"));
            }
            Ok(())
        } else {
            Ok(())
        }
    }

    async fn start_selected_step(self: &Arc<Self>, step_id: &str) -> Result<()> {
        let implementer = self.ensure_implementer().await?;
        // Persist ownership before the first execution request so a restart cannot lose the
        // Implementer identity after the step has already been admitted.
        self.save_state().await?;
        let plan = self
            .plan
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow::anyhow!("create a plan first"))?;
        let step = plan
            .steps
            .iter()
            .find(|step| step.definition.id == step_id)
            .ok_or_else(|| anyhow::anyhow!("unknown selected step"))?;
        let input = format!(
            "Objective: {}\nOverall acceptance: {}\nCurrent step {}: {}\nStep acceptance: {}\nBegin this step now. Work autonomously and return the actual result and evidence in your final reply.\n",
            plan.objective,
            plan.acceptance,
            step_id,
            step.definition.title,
            step.definition.acceptance,
        );
        let message = codex_extension_items::continuous_planning::MessageBatch {
            id: format!("plan:{}:{step_id}", plan.version),
            sender: codex_extension_items::continuous_planning::MessageRecipient::User,
            audience: codex_extension_items::continuous_planning::MessageRecipient::Implementer,
            messages: vec![
                codex_extension_items::continuous_planning::DirectedMessage {
                    id: format!("plan:{}:{step_id}:1", plan.version),
                    recipient:
                        codex_extension_items::continuous_planning::MessageRecipient::Implementer,
                    text: "Begin the selected step and return the actual result and evidence."
                        .into(),
                },
            ],
            final_answer: false,
        };
        let result = crate::control::deliver(
            &implementer,
            &input,
            codex_extension_items::ExtensionItem::ContinuousPlanningMessages(message),
        )
        .await?;
        match result {
            StartIfIdleSubmission::Started { .. } => Ok(()),
            StartIfIdleSubmission::NotSubmitted { reason } => {
                self.active.store(false, Ordering::SeqCst);
                anyhow::bail!("Implementer kickoff was rejected: {reason:?}")
            }
        }
    }

    pub(crate) async fn ensure_implementer(self: &Arc<Self>) -> Result<Arc<CodexThread>> {
        if let Some(implementer) = self.implementer.lock().await.as_ref() {
            return Ok(implementer.thread.clone());
        }
        let manager = self
            .manager
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("host stopped"))?;
        let parent = manager.get_thread(self.thread_id).await?;
        let mut config = parent.config().await.as_ref().clone();
        // The owned execution slot must not consume a level of user-configured delegation.
        config.agent_max_depth = config.agent_max_depth.saturating_add(1);
        config.features.disable(Feature::Goals)?;
        config.update_plan_enabled = false;
        config.experimental_request_user_input_enabled = false;
        let mut options = StartThreadOptions::new(config);
        options.history_mode = Some(codex_protocol::protocol::ThreadHistoryMode::Paginated);
        options.initial_history = InitialHistory::Forked(Vec::new());
        // Implementers share the existing child conversation lifecycle and navigation.
        let source = parent.session_source();
        let agent_path = source
            .get_agent_path()
            .unwrap_or_else(codex_protocol::AgentPath::root)
            .join("implementer")
            .map_err(anyhow::Error::msg)?;
        let depth = match source {
            SessionSource::SubAgent(SubAgentSource::ThreadSpawn { depth, .. }) => depth + 1,
            _ => 1,
        };
        options.session_source = Some(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: self.thread_id,
            depth,
            agent_path: Some(agent_path),
            agent_nickname: None,
            agent_role: Some("implementer".to_string()),
        }));
        let execution_id = ThreadId::new().to_string();
        options
            .thread_extension_init
            .insert(SupervisorSession::Implementer);
        options.thread_extension_init.insert(ImplementerBinding {
            runtime: Arc::downgrade(self),
            execution_id: execution_id.clone(),
            turn_steps: tokio::sync::Mutex::new(std::collections::VecDeque::new()),
        });
        let spawned = Box::pin(manager.spawn_child_session(self.thread_id, options)).await?;
        {
            let mut state = self.state.lock().await;
            state.implementer_thread_id = Some(spawned.thread_id.to_string());
            state.execution_id = Some(execution_id.clone());
        }
        *self.implementer.lock().await = Some(Implementer {
            thread: spawned.thread.clone(),
            execution_id,
        });
        Ok(spawned.thread)
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "pause and replacement serialize against Implementer startup"
    )]
    pub async fn suspend(&self) -> Result<()> {
        if let Some(error) = &self.restore_error {
            anyhow::bail!("{error}");
        }
        self.active.store(false, Ordering::SeqCst);
        let _control = self.control.lock().await;
        self.active.store(false, Ordering::SeqCst);
        let implementer = self
            .implementer
            .lock()
            .await
            .as_ref()
            .map(|implementer| implementer.thread.clone());
        if let Some(implementer) = implementer {
            self.interrupt_execution(&implementer).await?;
        }
        self.freeze("Execution paused").await
    }

    async fn interrupt_execution(&self, implementer: &CodexThread) -> Result<()> {
        if implementer.agent_status().await == AgentStatus::Shutdown {
            return Ok(());
        }
        // Stop the spawner first, then suspend delegated Supervisors even when their
        // conversation is idle while their Implementer is still running.
        interrupt_implementer(implementer).await?;
        if let Some(manager) = self.manager.upgrade() {
            let implementer_id = {
                let state = self.state.lock().await;
                ThreadId::from_string(
                    state
                        .implementer_thread_id
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("missing Implementer identity"))?,
                )?
            };
            for thread_id in manager
                .list_agent_subtree_thread_ids(implementer_id)
                .await?
            {
                if thread_id == implementer_id {
                    continue;
                }
                if let Ok(thread) = manager.get_thread(thread_id).await {
                    if let Some(runtime) = thread.thread_extension_data().get::<PlanRuntime>() {
                        Box::pin(runtime.suspend()).await?;
                    }
                    interrupt_implementer(&thread).await?;
                }
            }
        }
        Ok(())
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "timers and step states must be checkpointed atomically"
    )]
    pub(crate) async fn freeze(&self, reason: &str) -> Result<()> {
        let mut current = self.plan.lock().await;
        if let Some(mut plan) = current.clone() {
            let timestamp = now();
            for step in &mut plan.steps {
                if step.running_since.is_some() {
                    step.elapsed_seconds = step.elapsed_at(timestamp);
                }
                if matches!(
                    step.state,
                    PlanStepState::Running | PlanStepState::Reviewing
                ) {
                    step.running_since = None;
                    step.state = PlanStepState::Blocked;
                }
            }
            plan.reason = reason.to_string();
            crate::model::finish_revision(&mut plan, timestamp);
            self.db.append_thread_plan(self.thread_id, &plan).await?;
            *current = Some(plan.clone());
            (self.sink)(self.thread_id, plan);
        }
        drop(current);
        self.messages.lock().await.generation += 1;
        self.state.lock().await.paused = true;
        self.save_state().await?;
        if let Some(plan) = self.plan.lock().await.clone() {
            self.write_plan_file(&plan).await?;
        }
        Ok(())
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "resume only changes execution admission"
    )]
    pub async fn resume(&self) -> Result<()> {
        let _control = self.control.lock().await;
        ensure!(
            !self.stopped.load(Ordering::SeqCst),
            "Supervisor is stopped"
        );
        {
            let mut state = self.state.lock().await;
            ensure!(state.step_id.is_some(), "select a step first");
            ensure!(
                state
                    .token_budget
                    .is_none_or(|budget| state.total_tokens < budget),
                "task token budget reached; ask the user before increasing it"
            );
            state.paused = false;
        }
        self.active.store(true, Ordering::SeqCst);
        self.save_state().await
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "a replacement must not overlap the previous Implementer"
    )]
    pub async fn replace(self: &Arc<Self>) -> Result<()> {
        self.active.store(false, Ordering::SeqCst);
        let _control = self.control.lock().await;
        let old = self
            .implementer
            .lock()
            .await
            .as_ref()
            .map(|implementer| implementer.thread.clone());
        if let Some(old) = old {
            self.interrupt_execution(&old).await?;
            old.shutdown_and_wait().await?;
        }
        self.implementer.lock().await.take();
        self.freeze("Implementer context replaced").await?;
        self.ensure_implementer().await?;
        self.save_state().await
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "cancel must stop execution before committing the revision"
    )]
    pub async fn cancel(&self, version: i64, step_id: String, reason: String) -> Result<()> {
        let _control = self.control.lock().await;
        let mut current = self.plan.lock().await;
        let next = crate::model::apply(
            current.clone(),
            PlanActionRequest {
                version,
                operation: PlanAction::Cancel {
                    step_id: step_id.clone(),
                    reason,
                },
            },
            &self.thread_id.to_string(),
            now(),
        )?;
        if self.state.lock().await.step_id.as_ref() == Some(&step_id) {
            self.active.store(false, Ordering::SeqCst);
            let implementer = self
                .implementer
                .lock()
                .await
                .as_ref()
                .map(|implementer| implementer.thread.clone());
            if let Some(implementer) = implementer {
                self.interrupt_execution(&implementer).await?;
            }
            self.messages.lock().await.generation += 1;
            self.state.lock().await.paused = true;
        }
        self.db.append_thread_plan(self.thread_id, &next).await?;
        *current = Some(next.clone());
        (self.sink)(self.thread_id, next);
        let saved = current
            .clone()
            .ok_or_else(|| anyhow::anyhow!("plan was lost while cancelling the step"))?;
        drop(current);
        self.save_state().await?;
        self.write_plan_file(&saved).await
    }
}

pub(crate) async fn interrupt_implementer(thread: &CodexThread) -> Result<()> {
    thread.submit(Op::Interrupt).await?;
    for terminal in thread.list_background_terminals().await {
        if let Ok(process_id) = terminal.process_id.parse() {
            thread.terminate_background_terminal(process_id).await;
        }
    }
    while thread.agent_status().await == AgentStatus::Running {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

pub(crate) fn fragments(text: &str) -> Vec<codex_protocol::models::ResponseItem> {
    let mut remaining = text;
    let mut result = Vec::new();
    while !remaining.is_empty() {
        let end = remaining.floor_char_boundary(700.min(remaining.len()));
        result.push(ContextualUserFragment::into(
            ContinuousPlanningFragment::new(&remaining[..end]),
        ));
        remaining = &remaining[end..];
    }
    result
}

pub(crate) async fn deliver(
    thread: &CodexThread,
    text: &str,
    presentation: codex_extension_items::ExtensionItem,
) -> Result<StartIfIdleSubmission> {
    Ok(Box::pin(
        thread.start_turn_if_idle(TurnInputRequest::new(TurnInput::ContextualItems {
            items: fragments(text),
            presentation: Some(presentation),
        })),
    )
    .await?)
}
