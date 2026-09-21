use super::*;
use pretty_assertions::assert_eq;

fn initial() -> ContinuousPlanning {
    apply(
        None,
        PlanActionRequest {
            version: 0,
            operation: PlanAction::Create {
                objective: "Ship verified change".to_string(),
                acceptance: "Integration test passes".to_string(),
                stages: vec![PlanStage {
                    id: "s1".to_string(),
                    title: "Investigate".to_string(),
                    acceptance: "Choose a validated approach".to_string(),
                    assumptions: vec!["Existing API may be reusable".to_string()],
                    estimate_seconds: 60,
                }],
                steps: vec![PlanStepDefinition {
                    id: "a".to_string(),
                    stage_id: "s1".to_string(),
                    title: "Check behavior".to_string(),
                    acceptance: "Capture test output".to_string(),
                    dependencies: vec![],
                    estimate_seconds: 60,
                }],
            },
        },
        "root",
        /*now*/ 1000,
    )
    .unwrap()
}

fn advance(plan: ContinuousPlanning, operation: PlanAction, now: i64) -> ContinuousPlanning {
    let version = plan.version;
    apply(
        Some(plan),
        PlanActionRequest { version, operation },
        "root",
        now,
    )
    .unwrap()
}

#[test]
fn timing_survives_pause_and_submission_requires_review() {
    let plan = initial();
    let original = plan.steps[0].clone();
    let plan = advance(
        plan,
        PlanAction::Start {
            step_id: "a".to_string(),
        },
        /*now*/ 1010,
    );
    let plan = advance(
        plan,
        PlanAction::Pause {
            step_id: "a".to_string(),
            reason: "Awaiting input".to_string(),
        },
        /*now*/ 1025,
    );
    let plan = advance(
        plan,
        PlanAction::Start {
            step_id: "a".to_string(),
        },
        /*now*/ 1100,
    );
    let plan = advance(
        plan,
        PlanAction::Submit {
            step_id: "a".to_string(),
            evidence: vec!["test run #4 passed".to_string()],
        },
        /*now*/ 1110,
    );
    assert_eq!(
        plan.steps,
        vec![TimedPlanStep {
            state: PlanStepState::Reviewing,
            elapsed_seconds: 25,
            started_at: Some(1010),
            running_since: None,
            submitted_at: Some(1110),
            evidence: vec!["test run #4 passed".to_string()],
            ..original
        }]
    );
    assert_eq!(
        (plan.review, plan.next_review_at),
        (PlanReviewState::Due, 1110)
    );
}

#[test]
fn revision_preserves_baseline_and_actual_time() {
    let plan = advance(
        initial(),
        PlanAction::Start {
            step_id: "a".to_string(),
        },
        /*now*/ 1000,
    );
    let mut steps: Vec<_> = plan
        .steps
        .iter()
        .map(|step| step.definition.clone())
        .collect();
    steps[0].estimate_seconds = 120;
    let stages = plan.stages.clone();
    let mut expected = plan.steps[0].clone();
    expected.definition.estimate_seconds = 120;
    let plan = advance(
        plan,
        PlanAction::Revise {
            stages,
            steps,
            reason: "Experiment found an additional case".to_string(),
        },
        /*now*/ 1070,
    );
    assert_eq!(plan.steps, vec![expected]);
    assert_eq!(
        (
            plan.steps[0].elapsed_at(/*now*/ 1070),
            plan.estimated_completion_at
        ),
        (70, 1120)
    );
}

#[test]
fn completed_task_keeps_its_completion_time_after_resume() {
    let mut plan = initial();
    plan.steps[0].state = PlanStepState::Completed;
    plan.steps[0].completed_at = Some(1120);
    plan.updated_at = 1120;
    assert_eq!(
        plan.forecast_at(/*now*/ 4000),
        PlanForecast {
            steps: vec![1120],
            completed_at: 1120
        },
    );
}

#[test]
fn read_recovers_from_a_stale_version_without_modifying_the_plan() {
    let plan = initial();
    assert_eq!(
        apply(
            Some(plan.clone()),
            PlanActionRequest {
                version: 0,
                operation: PlanAction::Read { offset: 0 }
            },
            "root",
            /*now*/ 2000
        )
        .unwrap(),
        plan
    );
}

#[test]
fn unverified_dependency_prevents_execution() {
    let mut plan = initial();
    let mut definition = plan.steps[0].definition.clone();
    definition.id = "b".to_string();
    definition.dependencies = vec!["a".to_string()];
    let steps = vec![plan.steps[0].definition.clone(), definition];
    let stages = plan.stages.clone();
    plan = advance(
        plan,
        PlanAction::Revise {
            stages,
            steps,
            reason: "Add integration".to_string(),
        },
        /*now*/ 1001,
    );
    let version = plan.version;
    assert!(
        apply(
            Some(plan),
            PlanActionRequest {
                version,
                operation: PlanAction::Start {
                    step_id: "b".to_string()
                }
            },
            "root",
            /*now*/ 1002
        )
        .is_err()
    );
}

#[test]
fn future_stage_can_remain_unexpanded_until_evidence_exists() {
    let mut plan = initial();
    let mut future = plan.stages[0].clone();
    future.id = "s2".to_string();
    future.title = "Implement the validated approach".to_string();
    future.estimate_seconds = 300;
    let steps = plan
        .steps
        .iter()
        .map(|step| step.definition.clone())
        .collect();
    let mut stages = plan.stages.clone();
    stages.push(future);
    plan = advance(
        plan,
        PlanAction::Revise {
            stages,
            steps,
            reason: "Retain future uncertainty".to_string(),
        },
        /*now*/ 1000,
    );
    let plan = advance(
        plan,
        PlanAction::Start {
            step_id: "a".to_string(),
        },
        /*now*/ 1000,
    );
    assert_eq!(
        (
            plan.stages.len(),
            plan.steps.len(),
            plan.estimated_completion_at
        ),
        (2, 1, 1360)
    );
}

#[test]
fn forecast_respects_dependencies_between_different_executors() {
    let mut plan = initial();
    let mut parallel = plan.steps[0].clone();
    parallel.definition.id = "b".to_string();
    parallel.definition.estimate_seconds = 120;
    parallel.owner = "helper".to_string();
    plan.steps.push(parallel);
    finish_revision(&mut plan, /*now*/ 1000);
    assert_eq!(plan.estimated_completion_at, 1120);
    plan.steps[1].definition.dependencies.push("a".to_string());
    finish_revision(&mut plan, /*now*/ 1000);
    assert_eq!(plan.estimated_completion_at, 1180);
}

#[test]
fn forecast_waits_for_the_previous_stage_even_with_another_executor() {
    let mut plan = initial();
    let mut stage = plan.stages[0].clone();
    stage.id = "s2".to_string();
    plan.stages.push(stage);
    let mut step = plan.steps[0].clone();
    step.definition.id = "b".to_string();
    step.definition.stage_id = "s2".to_string();
    step.owner = "replacement".to_string();
    plan.steps.push(step);
    let forecast = plan.forecast_at(/*now*/ 1000);
    assert_eq!(
        forecast,
        PlanForecast {
            steps: vec![1060, 1120],
            completed_at: 1120
        }
    );
}

#[test]
fn cancelled_steps_do_not_reset_an_executors_remaining_work() {
    let mut plan = initial();
    let mut cancelled = plan.steps[0].clone();
    cancelled.definition.id = "b".to_string();
    cancelled.state = PlanStepState::Cancelled;
    let mut pending = plan.steps[0].clone();
    pending.definition.id = "c".to_string();
    plan.steps.extend([cancelled, pending]);
    assert_eq!(
        plan.forecast_at(/*now*/ 1000),
        PlanForecast {
            steps: vec![1060, 1000, 1120],
            completed_at: 1120
        }
    );
}
