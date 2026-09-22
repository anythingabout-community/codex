use crate::runtime::ImplementerBinding;
use crate::runtime::PlanRuntime;
use crate::runtime::PlanUpdateSink;
use crate::runtime::SupervisorUpdateSink;
use crate::runtime::now;
use crate::tool::SupervisorTool;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_core::context::ContextualUserFragment;
use codex_core::context::SupervisorRoleFragment;
use codex_extension_api::*;
use codex_protocol::ThreadId;
use codex_protocol::continuous_planning::PlanStepState;
use codex_state::StateRuntime;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

struct SupervisorExtension {
    db: Arc<StateRuntime>,
    manager: Weak<ThreadManager>,
    sink: PlanUpdateSink,
    state_sink: SupervisorUpdateSink,
}

pub fn install(
    registry: &mut ExtensionRegistryBuilder<Config>,
    db: Arc<StateRuntime>,
    manager: Weak<ThreadManager>,
    sink: PlanUpdateSink,
    state_sink: SupervisorUpdateSink,
) {
    let extension = Arc::new(SupervisorExtension {
        db,
        manager,
        sink,
        state_sink,
    });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.turn_lifecycle_contributor(extension.clone());
    registry.tool_lifecycle_contributor(extension.clone());
    registry.prompt_contributor(extension.clone());
    registry.token_usage_contributor(extension.clone());
    registry.turn_item_contributor(Arc::new(crate::message_items::MessageItems));
    registry.tool_contributor(extension);
}

impl ThreadLifecycleContributor<Config> for SupervisorExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if input.thread_store.get::<ImplementerBinding>().is_some()
                || input.session_source.get_agent_role().as_deref() == Some("implementer")
                || (input.session_source.is_non_root_agent()
                    && !matches!(
                        input.session_source,
                        codex_protocol::protocol::SessionSource::SubAgent(
                            codex_protocol::protocol::SubAgentSource::ThreadSpawn { .. }
                        )
                    ))
                || !input.persistent_thread_state_available
                || !input
                    .config
                    .features
                    .enabled(codex_features::Feature::ContinuousPlanning)
            {
                return;
            }
            let Ok(thread_id) = ThreadId::try_from(input.thread_store.level_id()) else {
                return;
            };
            let plan_result = self.db.read_thread_plan(thread_id).await;
            let state_result = self.db.read_supervisor(thread_id).await;
            let restore_error = plan_result
                .as_ref()
                .err()
                .or_else(|| state_result.as_ref().err())
                .map(|error| format!("Cannot restore Continuous Planning state: {error}. The stored data is preserved; execution is disabled."))
                .or_else(|| {
                    (matches!(&plan_result, Ok(Some(_)))
                        && matches!(&state_result, Ok(None)))
                    .then(|| "Cannot restore Continuous Planning without its ownership state. The stored plan is preserved; execution is disabled.".to_string())
                })
                .or_else(|| {
                    (matches!(&plan_result, Ok(None))
                        && matches!(&state_result, Ok(Some(_))))
                    .then(|| "Cannot restore Continuous Planning without its plan. The stored ownership state is preserved; execution is disabled.".to_string())
                });
            let plan = plan_result.ok().flatten();
            let restored = state_result.ok().flatten();
            let mut state = restored.clone().unwrap_or_default();
            // Execution runtimes do not survive process restart. Preserve their histories, never offline time.
            state.paused = restored.is_some() || restore_error.is_some();
            input.thread_store.insert(SupervisorSession::Supervisor);
            input.thread_store.insert(PlanRuntime {
                thread_id,
                restore_error,
                db: self.db.clone(),
                manager: self.manager.clone(),
                sink: self.sink.clone(),
                state_sink: self.state_sink.clone(),
                plan: tokio::sync::Mutex::new(plan),
                state: tokio::sync::Mutex::new(state),
                implementer: tokio::sync::Mutex::new(None),
                control: tokio::sync::Mutex::new(()),
                messages: tokio::sync::Mutex::new(crate::messages::MessageState::default()),
                evidence_review: tokio::sync::Mutex::new(None),
                active: AtomicBool::new(false),
                stopped: AtomicBool::new(false),
                activity_at: AtomicI64::new(now()),
                notified_at: AtomicI64::new(0),
            });
        })
    }
    fn on_thread_ready<'a>(
        &'a self,
        input: ThreadReadyInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(runtime) = input.thread_store.get::<PlanRuntime>() {
                if runtime.restore_error.is_some() {
                    return;
                }
                let restoring = {
                    let mut plan = runtime.plan.lock().await;
                    let restoring = plan.is_some();
                    if let Some(task) = plan.as_mut() {
                        for step in &mut task.steps {
                            if step.running_since.is_some() {
                                step.elapsed_seconds = step.elapsed_at(task.updated_at);
                                step.running_since = None;
                                step.state = PlanStepState::Blocked;
                            }
                        }
                    }
                    restoring
                };
                if restoring
                    && let Err(error) = runtime
                        .freeze("Supervisor restored; continue only on user instruction")
                        .await
                {
                    tracing::warn!(%error, "Supervisor checkpoint failed");
                }
                tokio::spawn(runtime.watch());
            }
        })
    }
    fn on_thread_idle<'a>(&'a self, input: ThreadIdleInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(binding) = input.thread_store.get::<ImplementerBinding>()
                && let Some(runtime) = binding.runtime.upgrade()
            {
                match runtime.retry_pending_delivery().await {
                    Ok(true) => {}
                    Ok(false) => {
                        if let Err(error) = runtime.flush_attention().await {
                            tracing::warn!(%error, "Supervisor attention delivery failed");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Implementer pending delivery retry failed");
                    }
                }
            } else if let Some(runtime) = input.thread_store.get::<PlanRuntime>()
                && let Err(error) = runtime.flush_attention().await
            {
                tracing::warn!(%error, "Supervisor attention delivery failed");
            }
        })
    }
    fn on_thread_stop<'a>(&'a self, input: ThreadStopInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(runtime) = input.thread_store.get::<PlanRuntime>() {
                if runtime.restore_error.is_some() {
                    return;
                }
                runtime.stopped.store(true, Ordering::SeqCst);
                let _ = runtime.suspend().await;
                let implementer = runtime.implementer.lock().await.take();
                if let Some(implementer) = implementer {
                    let _ = implementer.thread.shutdown_and_wait().await;
                }
            }
        })
    }
}

impl ContextContributor for SupervisorExtension {
    fn contribute_thread_context<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
    ) -> ExtensionFuture<'a, Vec<PromptFragment>> {
        Box::pin(async move {
            let text = match thread_store.get::<SupervisorSession>().as_deref() {
                Some(SupervisorSession::Supervisor) => crate::instructions::SUPERVISOR,
                Some(SupervisorSession::Implementer) => crate::instructions::IMPLEMENTER,
                None => return Vec::new(),
            };
            let mut remaining = text;
            let mut fragments = Vec::new();
            while !remaining.is_empty() {
                let cap = remaining.floor_char_boundary(700.min(remaining.len()));
                let end = if cap < remaining.len() {
                    remaining[..cap].rfind(". ").map_or(cap, |end| end + 2)
                } else {
                    cap
                };
                let fragment = SupervisorRoleFragment::new(&remaining[..end]);
                fragments.push(PromptFragment::developer_policy(
                    fragment.render(),
                    fragment.content_kind(),
                ));
                remaining = &remaining[end..];
            }
            fragments
        })
    }
}

impl TurnLifecycleContributor for SupervisorExtension {
    fn on_turn_start<'a>(&'a self, input: TurnStartInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(binding) = input.thread_store.get::<ImplementerBinding>()
                && let Some(runtime) = binding.runtime.upgrade()
            {
                let step_id = runtime.state.lock().await.step_id.clone();
                let Some(step_id) = step_id else {
                    return;
                };
                let turn = {
                    let mut messages = runtime.messages.lock().await;
                    let turn = crate::messages::ExecutionTurn {
                        turn_id: input.turn_id.to_string(),
                        execution_id: binding.execution_id.clone(),
                        step_id: step_id.clone(),
                        generation: messages.generation,
                    };
                    messages.turn = Some(turn.clone());
                    turn
                };
                input.turn_store.insert(turn);
                let title = runtime
                    .plan
                    .lock()
                    .await
                    .as_ref()
                    .and_then(|plan| plan.steps.iter().find(|step| step.definition.id == step_id))
                    .map(|step| step.definition.title.clone());
                if let Some(title) = title {
                    let mut turns = binding.turn_steps.lock().await;
                    if turns.len() == 64 {
                        turns.pop_front();
                    }
                    turns.push_back((input.turn_id.to_string(), step_id, title));
                }
            }
        })
    }
    fn on_turn_finished<'a>(&'a self, input: TurnFinishedInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(binding) = input.thread_store.get::<ImplementerBinding>()
                && let Some(runtime) = binding.runtime.upgrade()
                && let Some(turn) = input.turn_store.get::<crate::messages::ExecutionTurn>()
            {
                let turn = (*turn).clone();
                let report = input
                    .error
                    .is_none()
                    .then(|| {
                        input
                            .turn_store
                            .get::<crate::message_items::ImplementerReply>()
                            .map(|reply| (*reply).clone())
                    })
                    .flatten();
                if let Err(error) = runtime.finish_execution(&turn, report.as_ref()).await {
                    tracing::warn!(%error, "Implementer automatic report failed");
                }
            } else if let Some(runtime) = input.thread_store.get::<PlanRuntime>() {
                if input.error.is_some_and(|error| {
                    error.message.contains("Continuous Planning message error")
                }) {
                    let pause = {
                        let mut messages = runtime.messages.lock().await;
                        if messages.correction_attempted {
                            messages.feedback = None;
                            true
                        } else {
                            messages.correction_attempted = true;
                            messages.feedback = Some(format!(
                                "{}. The complete batch was rejected. Send one corrected complete batch; this is the only correction attempt.",
                                input
                                    .error
                                    .map(|error| error.message.as_str())
                                    .unwrap_or_default()
                            ));
                            false
                        }
                    };
                    if pause {
                        let _ = runtime.suspend().await;
                        return;
                    }
                }
                let feedback = runtime.messages.lock().await.feedback.take();
                if let Some(feedback) = feedback {
                    let feedback_runtime = runtime.clone();
                    tokio::spawn(async move {
                        if let Err(error) = feedback_runtime.feedback(&feedback).await {
                            tracing::warn!(%error, "Supervisor correction delivery failed");
                        } else if let Err(error) = feedback_runtime.flush_attention().await {
                            tracing::warn!(%error, "Supervisor correction retry failed");
                        }
                    });
                }
                let attention_runtime = runtime.clone();
                tokio::spawn(async move {
                    if let Err(error) = attention_runtime.flush_attention().await {
                        tracing::warn!(%error, "Supervisor attention delivery failed");
                    }
                });
            }
        })
    }

    fn on_turn_abort<'a>(&'a self, input: TurnAbortInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(runtime) = input.thread_store.get::<PlanRuntime>() {
                let _ = runtime.suspend().await;
            } else if let Some(binding) = input.thread_store.get::<ImplementerBinding>()
                && let Some(runtime) = binding.runtime.upgrade()
                && let Some(turn) = input.turn_store.get::<crate::messages::ExecutionTurn>()
            {
                let turn = (*turn).clone();
                tokio::spawn(async move {
                    let _ = runtime.finish_execution(&turn, None).await;
                });
            }
        })
    }
}

impl ToolLifecycleContributor for SupervisorExtension {
    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            if let Some(binding) = input.thread_store.get::<ImplementerBinding>()
                && let Some(runtime) = binding.runtime.upgrade()
                && runtime.implementer_matches(&binding.execution_id).await
            {
                runtime.activity_at.store(now(), Ordering::SeqCst);
            }
        })
    }
}

impl TokenUsageContributor for SupervisorExtension {
    fn on_token_usage<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        usage: &'a codex_protocol::protocol::TokenUsageInfo,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let runtime = thread_store.get::<PlanRuntime>().or_else(|| {
                thread_store
                    .get::<ImplementerBinding>()
                    .and_then(|b| b.runtime.upgrade())
            });
            if let Some(runtime) = runtime {
                if runtime.restore_error.is_some() {
                    return;
                }
                let exceeded = {
                    let mut state = runtime.state.lock().await;
                    state.total_tokens = state
                        .total_tokens
                        .saturating_add(usage.last_token_usage.total_tokens);
                    state
                        .token_budget
                        .is_some_and(|budget| state.total_tokens >= budget)
                };
                let _ = runtime.save_state().await;
                if exceeded {
                    tokio::spawn(async move {
                        let _ = runtime.suspend().await;
                        if let Some(manager) = runtime.manager.upgrade()
                            && let Ok(supervisor) = manager.get_thread(runtime.thread_id).await
                        {
                            let _ = supervisor
                                .submit(codex_protocol::protocol::Op::Interrupt)
                                .await;
                        }
                    });
                }
            }
        })
    }
}

impl ToolContributor for SupervisorExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>> {
        if let Some(runtime) = thread_store.get::<PlanRuntime>() {
            return vec![Arc::new(SupervisorTool(runtime))];
        }
        Vec::new()
    }
}
