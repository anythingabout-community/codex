use std::collections::HashSet;

use anyhow::Result;
use anyhow::ensure;
use codex_protocol::continuous_planning::*;

pub(crate) fn apply(
    previous: Option<ContinuousPlanning>,
    request: PlanActionRequest,
    owner: &str,
    now: i64,
) -> Result<ContinuousPlanning> {
    if matches!(&request.operation, PlanAction::Read { .. }) {
        return previous.ok_or_else(|| anyhow::anyhow!("no active plan"));
    }
    ensure!(
        request.version == previous.as_ref().map_or(0, |plan| plan.version),
        "stale version; read the plan and retry"
    );
    let mut plan = if let PlanAction::Create {
        objective,
        acceptance,
        stages,
        steps,
    } = &request.operation
    {
        ensure!(
            previous
                .as_ref()
                .is_none_or(|plan| plan.steps.iter().all(|step| matches!(
                    step.state,
                    PlanStepState::Completed | PlanStepState::Cancelled
                ))),
            "finish or cancel the existing plan before creating another task"
        );
        ensure!(
            previous
                .as_ref()
                .is_none_or(|plan| plan.stages.iter().all(|stage| plan
                    .steps
                    .iter()
                    .any(|step| step.definition.stage_id == stage.id))),
            "resolve unexpanded stages before creating another task"
        );
        ensure!(
            !objective.trim().is_empty() && !acceptance.trim().is_empty(),
            "objective and acceptance are required"
        );
        ContinuousPlanning {
            id: codex_protocol::ThreadId::new().to_string(),
            version: request.version,
            objective: objective.clone(),
            acceptance: acceptance.clone(),
            stages: stages.clone(),
            steps: steps
                .iter()
                .map(|definition| TimedPlanStep {
                    definition: definition.clone(),
                    state: PlanStepState::Pending,
                    owner: owner.to_string(),
                    initial_estimate_seconds: definition.estimate_seconds,
                    elapsed_seconds: 0,
                    started_at: None,
                    running_since: None,
                    submitted_at: None,
                    completed_at: None,
                    evidence: Vec::new(),
                })
                .collect(),
            review: PlanReviewState::Idle,
            updated_at: now,
            next_review_at: now + 300,
            estimated_completion_at: now,
            reason: "Initial plan".to_string(),
        }
    } else {
        previous.ok_or_else(|| anyhow::anyhow!("create a plan first"))?
    };
    let cancelling = matches!(&request.operation, PlanAction::Cancel { .. });
    match request.operation {
        PlanAction::Create { .. } => {}
        PlanAction::Read { .. } => return Ok(plan),
        PlanAction::Revise {
            stages,
            steps,
            reason,
        } => {
            ensure!(!reason.trim().is_empty(), "explain the revision");
            for old in &plan.steps {
                ensure!(
                    steps.iter().any(|new| new.id == old.definition.id),
                    "retain old steps; cancel rather than delete them"
                );
            }
            let mut next = Vec::new();
            for definition in steps {
                if let Some(old) = plan
                    .steps
                    .iter()
                    .find(|step| step.definition.id == definition.id)
                {
                    ensure!(
                        !matches!(
                            old.state,
                            PlanStepState::Completed | PlanStepState::Cancelled
                        ) || definition == old.definition,
                        "finished steps are immutable"
                    );
                    ensure!(
                        old.definition.stage_id == definition.stage_id,
                        "step stage identity is immutable"
                    );
                    next.push(TimedPlanStep {
                        definition,
                        ..old.clone()
                    });
                } else {
                    next.push(TimedPlanStep {
                        initial_estimate_seconds: definition.estimate_seconds,
                        definition,
                        state: PlanStepState::Pending,
                        owner: owner.to_string(),
                        elapsed_seconds: 0,
                        started_at: None,
                        running_since: None,
                        submitted_at: None,
                        completed_at: None,
                        evidence: Vec::new(),
                    });
                }
            }
            plan.steps = next;
            plan.stages = stages;
            plan.reason = reason;
            if plan.review == PlanReviewState::Unavailable {
                plan.review = PlanReviewState::Due;
                plan.next_review_at = now;
            }
        }
        PlanAction::Start { ref step_id }
        | PlanAction::Pause { ref step_id, .. }
        | PlanAction::Submit { ref step_id, .. }
        | PlanAction::Cancel { ref step_id, .. } => {
            let index = plan
                .steps
                .iter()
                .position(|step| step.definition.id == *step_id)
                .ok_or_else(|| anyhow::anyhow!("unknown step"))?;
            ensure!(
                plan.steps[index].owner == owner,
                "step belongs to another executor"
            );
            if matches!(&request.operation, PlanAction::Start { .. }) {
                ensure!(
                    plan.steps
                        .iter()
                        .all(|step| step.state != PlanStepState::Running || step.owner != owner),
                    "pause the running step first"
                );
                ensure!(
                    plan.steps[index]
                        .definition
                        .dependencies
                        .iter()
                        .all(|id| plan.steps.iter().any(|step| step.definition.id == *id
                            && step.state == PlanStepState::Completed)),
                    "dependencies must be verified first"
                );
                let stage_index = plan
                    .stages
                    .iter()
                    .position(|stage| stage.id == plan.steps[index].definition.stage_id)
                    .unwrap_or(0);
                for stage in &plan.stages[..stage_index] {
                    let prior: Vec<_> = plan
                        .steps
                        .iter()
                        .filter(|step| step.definition.stage_id == stage.id)
                        .collect();
                    ensure!(
                        !prior.is_empty()
                            && prior.iter().all(|step| matches!(
                                step.state,
                                PlanStepState::Completed | PlanStepState::Cancelled
                            )),
                        "verify earlier stages before advancing"
                    );
                }
            }
            let step = &mut plan.steps[index];
            ensure!(
                !matches!(
                    step.state,
                    PlanStepState::Completed | PlanStepState::Cancelled
                ),
                "finished steps are immutable"
            );
            step.elapsed_seconds = step.elapsed_at(now);
            step.running_since = None;
            match request.operation {
                PlanAction::Start { .. } => {
                    ensure!(
                        matches!(
                            step.state,
                            PlanStepState::Pending
                                | PlanStepState::Blocked
                                | PlanStepState::Reviewing
                        ),
                        "step cannot start in this state"
                    );
                    step.started_at.get_or_insert(now);
                    step.submitted_at = None;
                    step.evidence.clear();
                    step.running_since = Some(now);
                    step.state = PlanStepState::Running;
                    plan.next_review_at = now
                        + (step.definition.estimate_seconds - step.elapsed_seconds).clamp(5, 300);
                    plan.reason = format!("Started {}", step.definition.id);
                }
                PlanAction::Submit { evidence, .. } => {
                    ensure!(
                        step.state == PlanStepState::Running,
                        "only a running step may submit evidence"
                    );
                    ensure!(
                        !evidence.is_empty()
                            && evidence.len() <= 8
                            && evidence
                                .iter()
                                .all(|item| !item.trim().is_empty() && item.len() <= 1024),
                        "provide 1–8 bounded evidence references"
                    );
                    step.evidence = evidence;
                    step.submitted_at = Some(now);
                    step.state = PlanStepState::Reviewing;
                    plan.review = PlanReviewState::Due;
                    plan.next_review_at = now;
                    plan.reason = format!(
                        "Submitted {} for independent verification",
                        step.definition.id
                    );
                }
                PlanAction::Pause { reason, .. } | PlanAction::Cancel { reason, .. } => {
                    ensure!(!reason.trim().is_empty(), "explain the interruption");
                    step.state = if cancelling {
                        PlanStepState::Cancelled
                    } else {
                        PlanStepState::Blocked
                    };
                    plan.reason = reason;
                    plan.review = PlanReviewState::Due;
                    plan.next_review_at = now;
                }
                PlanAction::Create { .. } | PlanAction::Revise { .. } | PlanAction::Read { .. } => {
                    unreachable!()
                }
            }
        }
    }
    validate(&plan)?;
    finish_revision(&mut plan, now);
    Ok(plan)
}

fn validate(plan: &ContinuousPlanning) -> Result<()> {
    ensure!(
        plan.objective.len() <= 2048 && plan.acceptance.len() <= 2048 && plan.reason.len() <= 2048,
        "plan text too long"
    );
    ensure!(
        !plan.stages.is_empty()
            && plan.stages.len() <= 16
            && !plan.steps.is_empty()
            && plan.steps.len() <= 64,
        "use 1–16 stages and 1–64 steps"
    );
    let mut stages = HashSet::new();
    for stage in &plan.stages {
        ensure!(
            stages.insert(&stage.id) && !stage.id.is_empty() && stage.id.len() <= 64,
            "stage IDs must be unique and bounded"
        );
        ensure!(
            !stage.title.trim().is_empty()
                && stage.title.len() <= 128
                && !stage.title.chars().any(char::is_control)
                && !stage.acceptance.is_empty()
                && stage.acceptance.len() <= 2048,
            "invalid stage description"
        );
        ensure!(
            stage.estimate_seconds > 0 && stage.estimate_seconds <= 31_536_000,
            "invalid stage estimate"
        );
        ensure!(
            stage.assumptions.len() <= 8 && stage.assumptions.iter().all(|text| text.len() <= 512),
            "assumptions too large"
        );
    }
    let mut ids = HashSet::new();
    let mut previous_stage = 0;
    for step in &plan.steps {
        let definition = &step.definition;
        ensure!(stages.contains(&definition.stage_id), "unknown stage");
        let stage = plan
            .stages
            .iter()
            .position(|stage| stage.id == definition.stage_id)
            .unwrap_or(0);
        ensure!(stage >= previous_stage, "keep steps in stage order");
        previous_stage = stage;
        ensure!(
            definition.dependencies.len() <= 16
                && definition.dependencies.iter().all(|id| ids.contains(id)),
            "at most 16 dependencies must precede the step"
        );
        ensure!(
            !definition.id.is_empty() && definition.id.len() <= 64 && ids.insert(&definition.id),
            "step IDs must be unique and bounded"
        );
        ensure!(
            !definition.title.trim().is_empty()
                && definition.title.len() <= 128
                && !definition.title.chars().any(char::is_control)
                && !definition.acceptance.is_empty()
                && definition.acceptance.len() <= 2048,
            "invalid step description"
        );
        ensure!(
            definition.estimate_seconds > 0 && definition.estimate_seconds <= 31_536_000,
            "invalid step estimate"
        );
    }
    Ok(())
}

pub(crate) fn finish_revision(plan: &mut ContinuousPlanning, now: i64) {
    plan.version += 1;
    plan.updated_at = now;
    plan.estimated_completion_at = plan.forecast_at(now).completed_at;
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
