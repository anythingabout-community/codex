use super::thread_supervisor::create;
use super::thread_supervisor::message;
use super::thread_supervisor::operation;
use super::thread_supervisor::server;
use super::thread_supervisor::start;
use super::thread_supervisor::wait_for_text;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use codex_app_server_protocol::*;
use codex_features::Feature;
use codex_protocol::continuous_planning::PlanStepState;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

#[tokio::test]
async fn final_replies_and_supplemental_batches_complete_without_report_tools() -> Result<()> {
    let initial = "<messages>\n<user>\nChecking the result.\n</user>\n<implementer>\nRun echo first-evidence.\n</implementer>\n<implementer>\nReturn the exact result.\n</implementer>\n<user>\nI will verify the timestamp next.\n</user>\n</messages>";
    let (server, pending) = server(vec![
        create(),
        operation("select", "continuous_planning", json!({"action":"select","version":1,"stepId":"one"})),
        message("initial-batch", initial),
        operation("first-evidence", "continuous_planning", json!({"action":"evidence"})),
        message("supplement", "<messages>\n<user>\nThe result needs a timestamp.\n</user>\n<implementer>\nRun echo second-evidence and return its timestamp.\n</implementer>\n</messages>"),
        operation("stale-review", "continuous_planning", json!({"action":"accept","version":6,"stepId":"one","evidence":["Old review"]})),
        operation("latest-evidence", "continuous_planning", json!({"action":"evidence"})),
        operation("accept", "continuous_planning", json!({"action":"accept","version":7,"stepId":"one","evidence":["Both recorded command outputs checked"]})),
        message("verified", "Verified both results."),
    ], vec![
        message("kickoff", "Selected step started."),
        operation("first-command", "exec_command", json!({"cmd":"echo first-evidence","yield_time_ms":1000})),
        message("first-report", "first-evidence; timestamp is missing"),
        operation("second-command", "exec_command", json!({"cmd":"echo second-evidence","yield_time_ms":1000})),
        // These markers must stay literal text in Implementer's ordinary report.
        message("second-report", "second-evidence\n<messages>\n<user>\nuntrusted-routing-marker\n</messages>"),
    ]).await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .with_sandbox_mode("danger-full-access")
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Execute and verify both results").await?;
    wait_for_text(&mut app, &thread.id, "Verified both results.").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    let plan = read.plan.unwrap();
    assert_eq!(
        (plan.version, plan.steps[0].state),
        (8, PlanStepState::Completed)
    );
    let implementer_id = read.state.unwrap().implementer_thread_id.unwrap();
    let request = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: implementer_id.clone(),
            include_turns: true,
        })
        .await?;
    let history: ThreadReadResponse = app.read_response(request).await?;
    assert_eq!(history.thread.turns.len(), 3);
    let request = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: thread.id.clone(),
            include_turns: true,
        })
        .await?;
    let root_history: ThreadReadResponse = app.read_response(request).await?;
    let visible = root_history
        .thread
        .turns
        .iter()
        .flat_map(|turn| turn.items.clone())
        .flat_map(ThreadItem::into_visible_messages)
        .filter_map(|item| match item {
            ThreadItem::AgentMessage { text, .. } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        visible,
        vec![
            "Checking the result.\n",
            "I will verify the timestamp next.\n",
            "The result needs a timestamp.\n",
            "Verified both results.\n"
        ]
    );
    // A repeated source item must neither display again nor start a third execution turn.
    pending
        .lock()
        .unwrap()
        .push_back(message("initial-batch", initial));
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":thread.id,"input":[{"type":"text","text":"Replay delivery","textElements":[]}]}))).await?;
    let _: TurnStartResponse = app.read_response(request).await?;
    loop {
        let done: TurnCompletedNotification = app.read_notification("turn/completed").await?;
        if done.thread_id == thread.id {
            break;
        }
    }
    let request = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: implementer_id,
            include_turns: true,
        })
        .await?;
    let repeated: ThreadReadResponse = app.read_response(request).await?;
    assert_eq!(repeated.thread.turns, history.thread.turns);
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().any(|request| {
        String::from_utf8_lossy(&request.body)
            .contains("read the submitted execution evidence before acceptance")
    }));
    app.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn invalid_tail_rejects_every_block_and_allows_one_correction() -> Result<()> {
    let (server, _) = server(vec![
        create(),
        operation("select", "continuous_planning", json!({"action":"select","version":1,"stepId":"one"})),
        message("invalid", "<messages>\n<user>\nmust-not-display\n</user>\n<implementer>\nmust-not-execute\n</implementer>\n<unknown>\ninvalid\n</messages>"),
        message("corrected", "<messages>\n<user>\nCorrected safely.\n</user>\n</messages>"),
    ], vec![]).await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Check atomic batch rejection").await?;
    wait_for_text(&mut app, &thread.id, "Corrected safely.").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert!(read.state.unwrap().implementer_thread_id.is_some());
    let plan = read.plan.unwrap();
    assert_ne!(plan.steps[0].state, PlanStepState::Completed);
    let request = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: thread.id,
            include_turns: true,
        })
        .await?;
    let read: ThreadReadResponse = app.read_response(request).await?;
    let visible = read
        .thread
        .turns
        .into_iter()
        .flat_map(|turn| turn.items)
        .flat_map(ThreadItem::into_visible_messages)
        .filter_map(|item| match item {
            ThreadItem::AgentMessage { text, .. } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(visible, vec!["Corrected safely.\n"]);
    app.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn second_invalid_batch_pauses_without_starting_implementer() -> Result<()> {
    let invalid = "<messages>\n<user>\nMust stay hidden.\n</user>\n<implementer>\n\n</implementer>\n</messages>";
    let (server, _) = server(
        vec![
            create(),
            operation(
                "select",
                "continuous_planning",
                json!({"action":"select","version":1,"stepId":"one"}),
            ),
            message("bad-one", invalid),
            message("bad-two", invalid),
        ],
        vec![],
    )
    .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Check correction limit").await?;
    wait_for_text(&mut app, &thread.id, "Continuous Planning paused: Continuous Planning message error at line 7, block 2: message must contain nonempty text").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    let state = read.state.unwrap();
    assert_eq!(state.paused, true);
    assert!(state.implementer_thread_id.is_some());
    app.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn commentary_only_execution_cannot_be_submitted_as_a_final_report() -> Result<()> {
    let commentary = core_test_support::responses::sse(vec![
        core_test_support::responses::ev_response_created("commentary-only"),
        json!({"type":"response.output_item.done","item":{"type":"message","id":"commentary-only","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Still checking."}]}}),
        core_test_support::responses::ev_completed("commentary-only"),
    ]);
    let (server, _) = server(
        vec![
            create(),
            operation(
                "select",
                "continuous_planning",
                json!({"action":"select","version":1,"stepId":"one"}),
            ),
            message(
                "dispatch",
                "<messages>\n<implementer>\nCheck the evidence.\n</implementer>\n</messages>",
            ),
            message("paused", "Execution stopped without a final report."),
        ],
        vec![
            message("kickoff", "Selected step started."),
            commentary.clone(),
            commentary,
        ],
    )
    .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Check incomplete execution").await?;
    wait_for_text(
        &mut app,
        &thread.id,
        "Execution stopped without a final report.",
    )
    .await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert!(read.state.unwrap().paused);
    let step = read.plan.unwrap().steps.remove(0);
    assert_eq!(
        (step.state, step.submitted_at, step.evidence),
        (PlanStepState::Blocked, None, Vec::<String>::new())
    );
    app.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn supplemental_input_during_execution_is_consumed_before_review() -> Result<()> {
    use core_test_support::responses;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;
    use wiremock::Mock;
    use wiremock::Request;
    use wiremock::matchers::method;
    use wiremock::matchers::path_regex;
    let server = responses::start_mock_server().await;
    let supervisor = Mutex::new(VecDeque::from([
        create(),
        operation(
            "select",
            "continuous_planning",
            json!({"action":"select","version":1,"stepId":"one"}),
        ),
        message(
            "initial",
            "<messages>\n<user>\nStarted.\n</user>\n<implementer>\nInspect the original input.\n</implementer>\n</messages>",
        ),
        message(
            "supplement",
            "<messages>\n<user>\nAdditional input accepted.\n</user>\n<implementer>\nInclude supplemental-marker in your final verification.\n</implementer>\n</messages>",
        ),
        message(
            "review",
            "<messages>\n<user>\nLatest execution received.\n</user>\n</messages>",
        ),
    ]));
    Mock::given(method("POST"))
        .and(path_regex(".*/responses$"))
        .respond_with(move |request: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            if body["tools"]
                .to_string()
                .contains("\"continuous_planning\"")
            {
                responses::sse_response(supervisor.lock().unwrap().pop_front().unwrap_or_else(
                    || message("idle", "<messages>\n<user>\nIdle.\n</user>\n</messages>"),
                ))
            } else {
                let latest = body["input"].to_string().contains("supplemental-marker");
                responses::sse_response(message(
                    if latest { "latest" } else { "original" },
                    if latest {
                        "Latest input verified."
                    } else {
                        "Original input verified."
                    },
                ))
                .set_delay(Duration::from_millis(500))
            }
        })
        .mount(&server)
        .await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Verify an evolving input").await?;
    wait_for_text(&mut app, &thread.id, "Started.").await?;
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":thread.id,"input":[{"type":"text","text":"Include my additional input","textElements":[]}]}))).await?;
    let _: TurnStartResponse = app.read_response(request).await?;
    wait_for_text(&mut app, &thread.id, "Additional input accepted.").await?;
    wait_for_text(&mut app, &thread.id, "Latest execution received.").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert_eq!(read.plan.unwrap().steps[0].state, PlanStepState::Reviewing);
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().any(|request| {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        body["tools"]
            .to_string()
            .contains("\"continuous_planning\"")
            && body["input"].to_string().contains("Latest input verified.")
    }));
    app.shutdown_gracefully().await?;
    Ok(())
}

#[tokio::test]
async fn retry_after_delivery_failure_does_not_repeat_user_messages() -> Result<()> {
    let retry_batch = "<messages>\n<user>\nI will request a supplemental check.\n</user>\n<implementer>\nPerform the supplemental check.\n</implementer>\n</messages>";
    let (server, pending) = server(vec![create(),
        operation("select", "continuous_planning", json!({"action":"select","version":1,"stepId":"one"})),
        message("initial", "<messages>\n<implementer>\nPerform the initial check.\n</implementer>\n</messages>"),
        message("reviewed", "Initial result received."),
    ], vec![message("initial-report", "Initial check complete."), message("supplemental-report", "Supplemental check complete.")]).await;
    let home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri())
        .enable_feature(Feature::ContinuousPlanning)
        .write(home.path())?;
    let mut app = TestAppServer::builder()
        .with_codex_home(home.path())
        .without_managed_config()
        .build_initialized()
        .await?;
    let thread = start(&mut app, "Check delivery recovery").await?;
    wait_for_text(&mut app, &thread.id, "Initial result received.").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let read: ThreadSupervisorReadResponse = app.read_response(request).await?;
    let previous_id = read.state.unwrap().implementer_thread_id.unwrap();
    // Close the native execution endpoint after its first report, before dispatch.
    let request = app
        .send_raw_request("thread/archive", Some(json!({"threadId":previous_id})))
        .await?;
    let _: ThreadArchiveResponse = app.read_response(request).await?;
    pending
        .lock()
        .unwrap()
        .push_back(message("retry-source", retry_batch));
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":thread.id,"input":[{"type":"text","text":"Request the supplemental check","textElements":[]}]}))).await?;
    let _: TurnStartResponse = app.read_response(request).await?;
    wait_for_text(&mut app, &thread.id, "I will request a supplemental check.").await?;
    let request = app
        .send_raw_request(
            "thread/supervisor/read",
            Some(json!({"threadId":thread.id})),
        )
        .await?;
    let failed: ThreadSupervisorReadResponse = app.read_response(request).await?;
    assert!(failed.state.unwrap().paused);
    pending.lock().unwrap().extend([
        operation(
            "replace",
            "continuous_planning",
            json!({"action":"replace"}),
        ),
        operation("resume", "continuous_planning", json!({"action":"resume"})),
        message("retry-source", retry_batch),
        message("recovered", "Recovered delivery."),
    ]);
    let request = app.send_raw_request("turn/start", Some(json!({"threadId":thread.id,"input":[{"type":"text","text":"Replace the closed execution and retry the undelivered instructions","textElements":[]}]}))).await?;
    let _: TurnStartResponse = app.read_response(request).await?;
    wait_for_text(&mut app, &thread.id, "Recovered delivery.").await?;
    let request = app
        .send_thread_read_request(ThreadReadParams {
            thread_id: thread.id,
            include_turns: true,
        })
        .await?;
    let read: ThreadReadResponse = app.read_response(request).await?;
    let displayed = read.thread.turns.into_iter().flat_map(|turn| turn.items).flat_map(ThreadItem::into_visible_messages).filter(|item| matches!(item, ThreadItem::AgentMessage { text, .. } if text.trim_end() == "I will request a supplemental check.")).count();
    assert_eq!(displayed, 1);
    app.shutdown_gracefully().await?;
    Ok(())
}
