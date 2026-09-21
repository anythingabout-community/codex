use super::*;
use codex_utils_absolute_path::test_support::PathExt;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn supervisor_ownership_and_activity_survive_restart() -> anyhow::Result<()> {
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
    let state = SupervisorState {
        implementer_thread_id: Some(ThreadId::new().to_string()),
        execution_id: Some("execution-one".into()),
        step_id: Some("inspect".into()),
        revision: 1,
        paused: true,
        total_tokens: 120,
        token_budget: Some(1000),
        ..SupervisorState::default()
    };
    db.save_supervisor(thread, &state).await?;
    assert_eq!(
        db.append_supervisor_activity(thread, r#"{"type":"started"}"#)
            .await?,
        1
    );
    assert_eq!(
        db.append_supervisor_activity(thread, r#"{"type":"completed"}"#)
            .await?,
        2
    );
    drop(db);
    let reopened = StateRuntime::init(config, "test-provider".to_string()).await?;
    assert_eq!(
        reopened.read_supervisor(thread).await?,
        Some(SupervisorState {
            activity_sequence: 2,
            ..state
        })
    );
    assert_eq!(
        reopened
            .list_supervisor_activity(thread, /*after*/ 1, /*limit*/ 1)
            .await?,
        vec![(2, r#"{"type":"completed"}"#.to_string())]
    );
    Ok(())
}

#[tokio::test]
async fn delivery_outcomes_remain_separate_after_restart_and_retry() -> anyhow::Result<()> {
    let directory = crate::runtime::test_support::unique_temp_dir();
    let config = crate::SqliteConfig::new_for_testing(directory.as_path().abs());
    let db = StateRuntime::init(config.clone(), "test-provider".to_string()).await?;
    let thread = ThreadId::new();
    let partial = r#"{"userDelivered":true,"implementerDelivered":false,"error":"rejected"}"#;
    db.save_message_delivery(thread, "source", partial).await?;
    drop(db);
    let db = StateRuntime::init(config, "test-provider".to_string()).await?;
    assert_eq!(
        db.read_message_delivery(thread, "source").await?,
        Some(partial.to_string())
    );
    let completed = r#"{"userDelivered":true,"implementerDelivered":true,"error":null}"#;
    db.save_message_delivery(thread, "source", completed)
        .await?;
    assert_eq!(
        db.read_message_delivery(thread, "source").await?,
        Some(completed.to_string())
    );
    assert_eq!(
        db.read_message_delivery(ThreadId::new(), "source").await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn incompatible_ownership_state_is_reported_without_rewriting_data() -> anyhow::Result<()> {
    let directory = crate::runtime::test_support::unique_temp_dir();
    let config = crate::SqliteConfig::new_for_testing(directory.as_path().abs());
    let db = StateRuntime::init(config, "test-provider".to_string()).await?;
    let thread = ThreadId::new();
    db.upsert_thread(&crate::runtime::test_support::test_thread_metadata(
        &directory,
        thread,
        directory.clone(),
    ))
    .await?;
    let incompatible = r#"{"revision":1,"mainThreadId":"old-execution","executionId":"old","stepId":"one","paused":true,"updatedAt":0,"activitySequence":0,"totalTokens":0,"tokenBudget":null}"#;
    sqlx::query("INSERT INTO thread_supervisors (thread_id, snapshot) VALUES (?, ?)")
        .bind(thread.to_string())
        .bind(incompatible)
        .execute(db.pool.as_ref())
        .await?;
    assert!(db.read_supervisor(thread).await.is_err());
    let preserved: String =
        sqlx::query_scalar("SELECT snapshot FROM thread_supervisors WHERE thread_id = ?")
            .bind(thread.to_string())
            .fetch_one(db.pool.as_ref())
            .await?;
    assert_eq!(preserved, incompatible);
    Ok(())
}
