use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn supervisor_and_implementer_use_separate_conversation_views() {
    let (mut supervisor, mut supervisor_events, _) =
        make_chatwidget_manual(/*model_override*/ None).await;
    supervisor
        .config
        .features
        .enable(Feature::ContinuousPlanning)
        .expect("enable Supervisor");
    supervisor.set_supervisor_conversation_role(Some("supervisor"));
    replay_agent_message(
        &mut supervisor,
        "reply",
        "I am checking the evidence.",
        ReplayKind::ResumeInitialMessages,
    );
    let lines = drain_insert_history(&mut supervisor_events)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    insta::assert_snapshot!("supervisor_conversation", lines_to_single_string(&lines));
    assert_eq!(
        (
            supervisor.supervisor_enabled(),
            supervisor.collaboration_modes_enabled(),
            supervisor.goals_enabled()
        ),
        (true, false, false)
    );

    let (mut implementer, mut implementer_events, mut implementer_ops) =
        make_chatwidget_manual(/*model_override*/ None).await;
    implementer
        .config
        .features
        .enable(Feature::ContinuousPlanning)
        .expect("enable Supervisor");
    implementer.set_supervisor_conversation_role(Some("implementer"));
    let item = serde_json::from_value(serde_json::json!({
        "type":"commandExecution", "id":"command-one", "command":"echo verified", "cwd":test_path_display("/tmp"),
        "processId":null, "pluginId":null, "scriptPath":null, "source":"unifiedExecStartup", "status":"completed",
        "commandActions":[], "aggregatedOutput":"verified\n", "exitCode":0, "durationMs":5
    })).expect("command item");
    implementer.replay_thread_item(item, "execution".into(), ReplayKind::ResumeInitialMessages);
    implementer.flush_active_cell();
    let lines = drain_insert_history(&mut implementer_events)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert!(!lines.is_empty());
    insta::assert_snapshot!("implementer_conversation", lines_to_single_string(&lines));
    insta::assert_snapshot!(
        "implementer_read_only_composer",
        render_bottom_popup(&implementer, /*width*/ 100)
    );
    assert_eq!(
        (
            implementer.supervisor_enabled(),
            implementer.blocks_direct_input
        ),
        (false, true)
    );
    implementer
        .bottom_pane
        .set_composer_text("bypass Supervisor".into(), Vec::new(), Vec::new());
    implementer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_no_submit_op(&mut implementer_ops);
}

#[tokio::test]
async fn supervisor_task_list_above_composer() {
    let (mut chat, _, _) = make_chatwidget_manual(/*model_override*/ None).await;
    let plan = serde_json::from_value(serde_json::json!({
        "id": "task", "version": 1, "objective": "Ship", "acceptance": "Verified",
        "stages": [{"id": "S01", "title": "Ship", "acceptance": "Verified", "assumptions": [], "estimateSeconds": 60}],
        "steps": [{
            "definition": {"id": "A42", "stageId": "S01", "title": "验证实现路径", "acceptance": "Verified", "dependencies": [], "estimateSeconds": 60},
            "state": "reviewing", "owner": "root", "initialEstimateSeconds": 60,
            "elapsedSeconds": 25, "startedAt": 1000, "runningSince": null,
            "submittedAt": 1025, "completedAt": null, "evidence": []
        }],
        "review": "idle", "updatedAt": 1040, "nextReviewAt": 1070,
        "estimatedCompletionAt": 1100, "reason": "Verify results"
    })).expect("Continuous Planning");
    chat.bottom_pane
        .update_timed_plan(plan, "root".into(), /*observed_at*/ 1040);
    insta::assert_snapshot!(normalize_snapshot_paths(format!(
        "{}\n\n{}",
        render_bottom_popup(&chat, /*width*/ 80),
        render_bottom_popup(&chat, /*width*/ 32)
    )));
}

#[tokio::test]
async fn message_batches_render_identically_live_and_from_history() {
    let batch: codex_app_server_protocol::ThreadItem = serde_json::from_value(serde_json::json!({
        "type": "continuousPlanningMessages", "id": "batch", "sender": "supervisor", "audience": "user", "finalAnswer": true,
        "messages": [
            {"id":"batch:1", "recipient":"user", "text":"First result received.\n"},
            {"id":"batch:2", "recipient":"implementer", "text":"Private follow-up instruction.\n"},
            {"id":"batch:3", "recipient":"user", "text":"I will verify its timestamp.\n"}
        ]
    })).expect("batch");
    let mut rendered = Vec::new();
    for source in [
        ThreadItemRenderSource::Live,
        ThreadItemRenderSource::Replay(ReplayKind::ResumeInitialMessages),
    ] {
        let (mut chat, mut events, _) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.handle_thread_item(batch.clone(), "turn".into(), source);
        chat.flush_active_cell();
        let lines = drain_insert_history(&mut events)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        rendered.push(lines_to_single_string(&lines));
    }
    assert_eq!(rendered[0], rendered[1]);
    insta::assert_snapshot!("continuous_planning_message_batch", rendered[0]);
}
