use super::*;
use codex_protocol::continuous_planning::PlanReviewState;
use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn revisions_are_atomic_paginated_and_durable() -> anyhow::Result<()> {
    let directory = crate::runtime::test_support::unique_temp_dir();
    let config = crate::SqliteConfig::new_for_testing(directory.as_path().abs());
    let db = StateRuntime::init(config.clone(), "test-provider".to_string()).await?;
    let thread = ThreadId::new();
    db.upsert_thread(&crate::runtime::test_support::test_thread_metadata(
        &directory,
        thread,
        directory.clone(),
    ))
    .await?;
    let first = ContinuousPlanning {
        id: "task-one".to_string(),
        version: 1,
        objective: "Ship".to_string(),
        acceptance: "Test passes".to_string(),
        stages: vec![],
        steps: vec![],
        review: PlanReviewState::Idle,
        updated_at: 1000,
        next_review_at: 1060,
        estimated_completion_at: 1300,
        reason: "Initial".to_string(),
    };
    db.append_thread_plan(thread, &first).await?;
    assert!(db.append_thread_plan(thread, &first).await.is_err());
    let second = ContinuousPlanning {
        version: 2,
        reason: "Revised after evidence".to_string(),
        ..first.clone()
    };
    db.append_thread_plan(thread, &second).await?;
    assert_eq!(
        db.list_thread_plan_history(thread, i64::MAX, /*limit*/ 1)
            .await?,
        vec![second.clone()]
    );
    assert_eq!(
        db.list_thread_plan_history(thread, /*before*/ 2, /*limit*/ 1)
            .await?,
        vec![first]
    );
    assert_eq!(db.read_thread_plan(ThreadId::new()).await?, None);
    db.checkpoint_thread_plan(thread, /*version*/ 2, /*now*/ 1029)
        .await?;
    assert_eq!(
        db.list_thread_plan_history(thread, i64::MAX, /*limit*/ 1)
            .await?,
        vec![second.clone()]
    );
    drop(db);
    let reopened = StateRuntime::init(config, "test-provider".to_string()).await?;
    assert_eq!(
        reopened.read_thread_plan(thread).await?,
        Some(ContinuousPlanning {
            updated_at: 1029,
            ..second
        })
    );
    Ok(())
}
