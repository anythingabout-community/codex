use crate::actions::SupervisorAction;
use crate::runtime::PlanRuntime;
use codex_extension_api::*;
use codex_protocol::continuous_planning::PlanAction;
use codex_protocol::continuous_planning::PlanActionRequest;
use serde_json::json;
use std::sync::Arc;

pub(crate) struct SupervisorTool(pub Arc<PlanRuntime>);

impl<'call> ToolExecutor<ToolCall<'call>> for SupervisorTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("continuous_planning")
    }
    fn spec(&self) -> ToolSpec {
        specification::<SupervisorAction>(
            "continuous_planning",
            "Manage Continuous Planning: create or revise the staged plan, select a step, pause/resume execution, replace the Implementer context, inspect evidence, and accept verified work. Select does not start execution. Send instructions in ordinary message batches, never through tools. Use current versions for mutations. Reports arrive automatically; read evidence before acceptance.",
        )
    }
    fn handle<'a>(&'a self, call: ToolCall<'call>) -> ToolExecutorFuture<'a>
    where
        'call: 'a,
    {
        Box::pin(async move {
            let args = call.function_arguments()?;
            if args.len() > 64 * 1024 {
                return Err(FunctionCallError::RespondToModel(
                    "request too large".into(),
                ));
            }
            let action: SupervisorAction = serde_json::from_str(args)
                .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
            let value = self
                .execute(action)
                .await
                .map_err(|e| FunctionCallError::RespondToModel(e.to_string()))?;
            Ok(Box::new(JsonToolOutput::new(value)) as Box<dyn ToolOutput>)
        })
    }
}

impl SupervisorTool {
    async fn execute(&self, action: SupervisorAction) -> anyhow::Result<serde_json::Value> {
        let runtime = &self.0;
        if let Some(error) = &runtime.restore_error {
            anyhow::bail!("{error}");
        }
        match action {
            SupervisorAction::Read { offset } => {
                let plan = runtime.plan.lock().await.clone();
                let state = runtime.state.lock().await.clone();
                let legacy_goal = if plan.is_none() {
                    runtime.db.thread_goals().get_thread_goal(runtime.thread_id).await?.map(|goal| json!({"objective":goal.objective, "elapsedSeconds":goal.time_used_seconds, "tokensUsed":goal.tokens_used, "tokenBudget":goal.token_budget}))
                } else {
                    None
                };
                return page(
                    &json!({ "task": plan, "execution": state, "legacyGoal": legacy_goal })
                        .to_string(),
                    offset,
                );
            }
            SupervisorAction::Evidence { offset } => {
                let evidence = if offset == 0 {
                    runtime.evidence().await?
                } else {
                    runtime
                        .evidence_review
                        .lock()
                        .await
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("read evidence from offset zero first"))?
                        .1
                        .clone()
                };
                return page(&evidence, offset);
            }
            SupervisorAction::Create {
                version,
                objective,
                acceptance,
                stages,
                steps,
            } => {
                runtime
                    .update_plan(PlanActionRequest {
                        version,
                        operation: PlanAction::Create {
                            objective,
                            acceptance,
                            stages,
                            steps,
                        },
                    })
                    .await?;
            }
            SupervisorAction::Revise {
                version,
                stages,
                steps,
                reason,
            } => {
                runtime
                    .update_plan(PlanActionRequest {
                        version,
                        operation: PlanAction::Revise {
                            stages,
                            steps,
                            reason,
                        },
                    })
                    .await?;
            }
            SupervisorAction::Select { version, step_id } => {
                runtime.select(version, step_id).await?
            }
            SupervisorAction::Pause { reason } => {
                anyhow::ensure!(!reason.trim().is_empty(), "explain the pause");
                runtime.suspend().await?;
            }
            SupervisorAction::Resume {} => runtime.resume().await?,
            SupervisorAction::Replace {} => runtime.replace().await?,
            SupervisorAction::Budget { tokens } => {
                anyhow::ensure!(
                    tokens.is_none_or(|tokens| tokens > 0),
                    "provide a positive total budget or null to remove an explicitly lifted limit"
                );
                runtime.state.lock().await.token_budget = tokens;
                runtime.save_state().await?;
            }
            SupervisorAction::Accept {
                version,
                step_id,
                evidence,
            } => runtime.accept(version, &step_id, evidence).await?,
            SupervisorAction::Cancel {
                version,
                step_id,
                reason,
            } => {
                runtime.cancel(version, step_id, reason).await?;
            }
        }
        let plan = runtime.plan.lock().await;
        Ok(
            json!({ "version": plan.as_ref().map(|p| p.version), "steps": plan.as_ref().map(|p| p.steps.iter().filter(|s| !matches!(s.state, codex_protocol::continuous_planning::PlanStepState::Completed | codex_protocol::continuous_planning::PlanStepState::Cancelled)).take(3).map(|s| json!({"id": s.definition.id, "state": s.state})).collect::<Vec<_>>()) }),
        )
    }
}

#[expect(
    clippy::expect_used,
    reason = "schemas are generated from static Rust request types"
)]
fn specification<T: schemars::JsonSchema>(name: &str, description: &str) -> ToolSpec {
    let schema = serde_json::to_value(schemars::schema_for!(T)).expect("schema serializes");
    ToolSpec::Function(ResponsesApiTool {
        name: name.into(),
        description: description.into(),
        strict: false,
        defer_loading: None,
        parameters: parse_tool_input_schema(&schema).expect("valid schema"),
        output_schema: None,
    })
}

fn page(text: &str, offset: usize) -> anyhow::Result<serde_json::Value> {
    anyhow::ensure!(
        offset <= text.len() && text.is_char_boundary(offset),
        "invalid page offset"
    );
    let end = text.floor_char_boundary(offset.saturating_add(600).min(text.len()));
    Ok(json!({"text": &text[offset..end], "nextOffset": (end < text.len()).then_some(end)}))
}
