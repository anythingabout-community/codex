//! Durable task state and bounded, event-driven delivery to the user-facing Supervisor.

use anyhow::Result;
use anyhow::ensure;
use codex_core::CodexThread;
use codex_core::ThreadManager;
use codex_protocol::ThreadId;
use codex_protocol::continuous_planning::*;
use codex_protocol::supervisor::SupervisorState;
use codex_state::StateRuntime;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tokio::sync::Mutex;

pub type PlanUpdateSink = Arc<dyn Fn(ThreadId, ContinuousPlanning) + Send + Sync>;
pub type SupervisorUpdateSink = Arc<dyn Fn(ThreadId, SupervisorState) + Send + Sync>;

pub(crate) struct Implementer {
    pub thread: Arc<CodexThread>,
    pub execution_id: String,
}

/// Host binding; a Implementer cannot select another Supervisor or execution epoch.
pub struct ImplementerBinding {
    pub(crate) runtime: Weak<PlanRuntime>,
    pub(crate) execution_id: String,
    pub(crate) turn_steps: Mutex<std::collections::VecDeque<(String, String, String)>>,
}

pub(crate) struct PlanRuntime {
    pub thread_id: ThreadId,
    pub restore_error: Option<String>,
    pub db: Arc<StateRuntime>,
    pub manager: Weak<ThreadManager>,
    pub sink: PlanUpdateSink,
    pub state_sink: SupervisorUpdateSink,
    pub plan: Mutex<Option<ContinuousPlanning>>,
    pub state: Mutex<SupervisorState>,
    pub implementer: Mutex<Option<Implementer>>,
    pub control: Mutex<()>,
    pub messages: Mutex<crate::messages::MessageState>,
    pub evidence_review: Mutex<Option<(i64, String)>>,
    pub active: AtomicBool,
    pub stopped: AtomicBool,
    pub activity_at: AtomicI64,
    pub notified_at: AtomicI64,
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs() as i64)
}

impl PlanRuntime {
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "a durable revision must serialize with other task mutations"
    )]
    pub async fn update_plan(&self, request: PlanActionRequest) -> Result<ContinuousPlanning> {
        ensure!(
            matches!(
                request.operation,
                PlanAction::Create { .. } | PlanAction::Revise { .. } | PlanAction::Read { .. }
            ),
            "use Implementer controls for execution"
        );
        let mut current = self.plan.lock().await;
        let plan =
            crate::model::apply(current.clone(), request, &self.thread_id.to_string(), now())?;
        if current
            .as_ref()
            .is_none_or(|old| old.version != plan.version)
        {
            self.db.append_thread_plan(self.thread_id, &plan).await?;
            *current = Some(plan.clone());
            (self.sink)(self.thread_id, plan.clone());
        }
        drop(current);
        self.write_plan_file(&plan).await?;
        Ok(plan)
    }

    /// Writes the host-owned plan projection where a human can review and edit it between turns.
    pub(crate) async fn write_plan_file(&self, plan: &ContinuousPlanning) -> Result<()> {
        let configured_path = self.state.lock().await.plan_path.clone();
        let path = if let Some(path) = configured_path {
            PathBuf::from(path)
        } else {
            let manager = self
                .manager
                .upgrade()
                .ok_or_else(|| anyhow::anyhow!("host stopped"))?;
            let thread = manager.get_thread(self.thread_id).await?;
            let cwd = thread.config().await.cwd.to_path_buf();
            let path = cwd
                .join(".agents")
                .join("plans")
                .join(format!("continuous-{}.md", plan.id));
            {
                let mut state = self.state.lock().await;
                state.plan_path = Some(path.to_string_lossy().into_owned());
            }
            self.save_state().await?;
            path
        };
        let mut markdown = String::new();
        let _ = writeln!(markdown, "# Continuous Plan");
        let _ = writeln!(markdown, "\n- **Plan ID:** `{}`", plan.id);
        let _ = writeln!(markdown, "- **Revision:** {}", plan.version);
        let _ = writeln!(markdown, "- **Updated:** {}", plan.updated_at);
        let _ = writeln!(markdown, "\n## Objective\n\n{}", plan.objective);
        let _ = writeln!(markdown, "\n## Acceptance\n\n{}", plan.acceptance);
        let _ = writeln!(markdown, "\n## Current Reason\n\n{}", plan.reason);
        for stage in &plan.stages {
            let _ = writeln!(markdown, "\n## {} ({})", stage.title, stage.id);
            let _ = writeln!(markdown, "\n{}", stage.acceptance);
            for step in plan
                .steps
                .iter()
                .filter(|step| step.definition.stage_id == stage.id)
            {
                let marker = match step.state {
                    PlanStepState::Completed => "x",
                    PlanStepState::Cancelled => "-",
                    _ => " ",
                };
                let _ = writeln!(
                    markdown,
                    "\n- [{}] `{}` **{:?}**: {}\n  - Acceptance: {}\n  - Estimate: {} seconds\n  - Dependencies: {}",
                    marker,
                    step.definition.id,
                    step.state,
                    step.definition.title,
                    step.definition.acceptance,
                    step.definition.estimate_seconds,
                    if step.definition.dependencies.is_empty() {
                        "none".to_string()
                    } else {
                        step.definition.dependencies.join(", ")
                    }
                );
            }
        }
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, markdown).await?;
        Ok(())
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "state writes are ordered with state notifications"
    )]
    pub async fn save_state(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.updated_at = now();
        state.revision += 1;
        self.db.save_supervisor(self.thread_id, &state).await?;
        (self.state_sink)(self.thread_id, state.clone());
        Ok(())
    }

    pub async fn implementer_matches(&self, execution_id: &str) -> bool {
        self.implementer
            .lock()
            .await
            .as_ref()
            .is_some_and(|implementer| implementer.execution_id == execution_id)
    }

    /// A bounded notification wakes the existing conversation; it never creates a reviewer.
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "host wakeups serialize with pause and replacement"
    )]
    pub async fn notify(&self, reason: &str) -> Result<()> {
        let _control = self.control.lock().await;
        if !self.active.load(Ordering::SeqCst) || self.stopped.load(Ordering::SeqCst) {
            return Ok(());
        }
        let text = format!(
            "Supervisor execution update: {reason}. Inspect Continuous Planning and execution evidence. This is host context, not new user authorization."
        );
        let mut state = self.state.lock().await;
        if state.pending_attention.is_none() {
            state.pending_attention = Some(text);
        }
        drop(state);
        self.save_state().await?;
        self.flush_attention().await
    }

    pub async fn watch(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut checkpoint = now();
        while !self.stopped.load(Ordering::SeqCst) {
            ticker.tick().await;
            if !self.active.load(Ordering::SeqCst) {
                continue;
            }
            let snapshot = self.plan.lock().await.clone();
            let Some(plan) = snapshot else { continue };
            let timestamp = now();
            let due = plan.next_review_at <= timestamp
                && timestamp - self.notified_at.load(Ordering::SeqCst) >= 300
                && plan
                    .steps
                    .iter()
                    .any(|step| step.state == PlanStepState::Running)
                && (timestamp - self.activity_at.load(Ordering::SeqCst) >= 300
                    || plan.steps.iter().any(|step| {
                        step.state == PlanStepState::Running
                            && step.elapsed_at(timestamp) >= step.definition.estimate_seconds
                    }));
            if due {
                self.notified_at.store(timestamp, Ordering::SeqCst);
                let _ = self.notify("Check the current estimate and recent progress; elapsed time alone does not indicate failure").await;
            }
            if timestamp - checkpoint >= 30 {
                let _ = self
                    .db
                    .checkpoint_thread_plan(self.thread_id, plan.version, timestamp)
                    .await;
                checkpoint = timestamp;
            }
        }
    }
}
