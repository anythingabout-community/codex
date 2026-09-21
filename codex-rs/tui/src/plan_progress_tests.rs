use super::*;
use pretty_assertions::assert_eq;

fn fixture() -> ContinuousPlanning {
    ContinuousPlanning {
        id: "task-one".to_string(),
        version: 1,
        objective: "Ship".to_string(),
        acceptance: "Verified".to_string(),
        stages: vec![PlanStage {
            id: "S01".into(),
            title: "Ship".into(),
            acceptance: "Verified".into(),
            assumptions: vec![],
            estimate_seconds: 180,
        }],
        steps: ["Inspect", "Implement", "Test"]
            .into_iter()
            .enumerate()
            .map(|(index, title)| TimedPlanStep {
                definition: PlanStepDefinition {
                    id: format!("A{:02}", index + 1),
                    stage_id: "S01".to_string(),
                    title: title.to_string(),
                    acceptance: "Check output".to_string(),
                    dependencies: vec![],
                    estimate_seconds: 60,
                },
                state: if index == 0 {
                    PlanStepState::Completed
                } else if index == 1 {
                    PlanStepState::Running
                } else {
                    PlanStepState::Pending
                },
                owner: "root".to_string(),
                initial_estimate_seconds: 60,
                elapsed_seconds: if index == 0 { 25 } else { 0 },
                started_at: if index < 2 { Some(1000) } else { None },
                running_since: if index == 1 { Some(1010) } else { None },
                submitted_at: if index == 0 { Some(1020) } else { None },
                completed_at: if index == 0 { Some(1025) } else { None },
                evidence: vec![],
            })
            .collect(),
        review: PlanReviewState::Idle,
        updated_at: 1040,
        next_review_at: 1070,
        estimated_completion_at: 1130,
        reason: "Updated estimate".to_string(),
    }
}

fn progress(plan: ContinuousPlanning) -> PlanProgress {
    let observed_at = plan.updated_at;
    let mut progress = PlanProgress::new(
        plan,
        "root".to_string(),
        observed_at,
        FrameRequester::test_dummy(),
    );
    progress.received = Instant::now() + Duration::from_secs(/*secs*/ 3600);
    progress
}

fn rendered(progress: &PlanProgress, width: u16) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(
        /*x*/ 0,
        /*y*/ 0,
        width,
        progress.desired_height(width),
    ));
    progress.render(buf.area, &mut buf);
    buf
}

fn snapshot(progress: &PlanProgress, width: u16) -> String {
    buffer_text(&rendered(progress, width))
}

fn buffer_text(buf: &Buffer) -> String {
    (buf.area.y..buf.area.bottom())
        .map(|y| {
            let mut line = String::new();
            let mut x = buf.area.x;
            while x < buf.area.right() {
                let symbol = buf[(x, y)].symbol();
                line.push_str(symbol);
                x += symbol.width().max(/*other*/ 1) as u16;
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn task_list_wide_and_narrow() {
    let mut plan = fixture();
    plan.steps[1].definition.title = "验证实现路径".into();
    let progress = progress(plan);
    insta::assert_snapshot!(
        [80, 46, 45, 35, 34, 28, 27, 12]
            .into_iter()
            .map(|width| format!("{width} columns\n{}", snapshot(&progress, width)))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    let buf = rendered(&progress, /*width*/ 80);
    assert_eq!(
        (
            buf[(10, 1)].fg,
            buf[(10, 1)].modifier,
            buf[(6, 1)].modifier,
            buf[(10, 2)].fg,
            buf[(10, 2)].modifier
        ),
        (
            ratatui::style::Color::Reset,
            ratatui::style::Modifier::DIM | ratatui::style::Modifier::CROSSED_OUT,
            ratatui::style::Modifier::DIM,
            ratatui::style::Color::Cyan,
            ratatui::style::Modifier::BOLD
        )
    );
}

#[test]
fn timing_degrades_at_measured_boundaries_and_recovers() {
    use super::layout::Timing::*;
    let mut plan = fixture();
    plan.steps[1].definition.title = "验证实现路径".into();
    let progress = progress(plan);
    let entries = progress.entries();
    assert_eq!(
        [46, 45, 35, 34, 28, 27, 28, 35, 46]
            .map(|width| super::layout::RowLayout::new(&entries, width).timing),
        [
            Bar, Arrow, Arrow, Estimate, Estimate, Hidden, Estimate, Arrow, Bar
        ]
    );
    for width in [46, 45, 35, 34, 28, 27] {
        let rows = snapshot(&progress, width);
        let running = rows.lines().nth(2).unwrap();
        assert!(running.contains("A02 验证实现路径"));
        if (35..46).contains(&width) {
            assert!(running.ends_with("0:30 → 1:00"));
        }
        if (28..35).contains(&width) {
            assert!(running.ends_with("1:00"));
        }
    }
}

#[test]
fn overview_is_continuous_with_aligned_ids_and_minimum_widths() {
    let mut plan = fixture();
    plan.steps[2].definition.id = "legacy-long-id".into();
    let progress = progress(plan);
    for width in [19, 26, 32, 80] {
        let entries = progress.entries();
        let segments = super::layout::segments(&entries, width - 4);
        let buf = rendered(&progress, width);
        let mut x = 4;
        let y = buf.area.height - 2;
        for (position, (index, size)) in segments.iter().copied().enumerate() {
            assert!(usize::from(size) >= (entries[index].id.width() + 1).max(4));
            assert_eq!(buf[(x, y)].symbol(), &entries[index].id[..1]);
            if position > 0 {
                assert!(matches!(buf[(x, y + 1)].symbol(), "┼" | "┿"));
            }
            x += size;
        }
        assert_eq!(x, width);
        assert!((4..width).all(|x| buf[(x, y + 1)].symbol() != " "));
    }
    assert!(super::layout::segments(&progress.entries(), /*width*/ 14).is_empty());
    insta::assert_snapshot!(snapshot(&progress, /*width*/ 32));
}

#[test]
fn logarithmic_widths_and_latest_whole_segments() {
    let mut plan = fixture();
    plan.steps[0].state = PlanStepState::Pending;
    plan.steps[0].elapsed_seconds = 0;
    for (step, seconds) in plan.steps.iter_mut().zip([1, 100, 10000]) {
        step.definition.estimate_seconds = seconds;
    }
    let progress = progress(plan);
    let sizes = super::layout::segments(&progress.entries(), /*width*/ 80);
    assert!(sizes[0].1 < sizes[1].1 && sizes[1].1 < sizes[2].1 && sizes[2].1 < 3 * sizes[1].1);
    assert_eq!(
        super::layout::segments(&progress.entries(), /*width*/ 8),
        vec![(1, 4), (2, 4)]
    );
    assert_eq!(
        super::layout::segments(&progress.entries(), /*width*/ 4),
        vec![(2, 4)]
    );
    insta::assert_snapshot!(snapshot(&progress, /*width*/ 12));
}

#[test]
fn identities_survive_revision_and_stage_aggregation() {
    let mut plan = fixture();
    let mut stage = plan.stages[0].clone();
    stage.id = "S09".into();
    stage.title = "Next".into();
    plan.stages.push(stage);
    for step in &mut plan.steps {
        step.state = PlanStepState::Completed;
        step.running_since = None;
    }
    for index in 3..8 {
        let mut step = plan.steps[2].clone();
        step.definition.id = format!("A{:02}", index + 1);
        step.definition.stage_id = "S09".into();
        step.state = PlanStepState::Pending;
        plan.steps.push(step);
    }
    let mut progress = progress(plan.clone());
    assert_eq!(progress.entries()[0].id, "S01");
    assert_eq!(progress.entries()[0].title, "Ship (3 tasks)");
    assert_eq!(progress.plan, plan);
    plan.version += 1;
    progress.update(plan, /*observed_at*/ 1040);
    assert_eq!(progress.desired_height(/*width*/ 80), 9);
    insta::assert_snapshot!(snapshot(&progress, /*width*/ 64));
}

#[test]
fn keyboard_details_revision_and_restoration_use_original_ids() {
    let mut plan = fixture();
    plan.steps[1].definition.id = "legacy-task-42".into();
    let mut progress = progress(plan.clone());
    rendered(&progress, /*width*/ 80);
    progress.toggle_focus();
    assert!(progress.details().starts_with("legacy-task-42 Implement"));
    plan.version += 1;
    plan.steps[1].definition.title = "Revised".into();
    plan.steps.reverse();
    progress.update(plan.clone(), /*observed_at*/ 1100);
    progress.received = Instant::now() + Duration::from_secs(/*secs*/ 3600);
    rendered(&progress, /*width*/ 80);
    assert!(progress.details().starts_with("legacy-task-42 Revised"));
    assert_eq!(progress.plan.steps[1].elapsed_at(progress.timestamp()), 90);
    insta::assert_snapshot!(snapshot(&progress, /*width*/ 80));
    let restored = super::tests::progress(plan.clone());
    assert_eq!(
        restored
            .entries()
            .iter()
            .map(|entry| &entry.id)
            .collect::<Vec<_>>(),
        plan.steps
            .iter()
            .map(|step| &step.definition.id)
            .collect::<Vec<_>>()
    );
    assert!(progress.handle_action(ListAction::JumpTop));
    assert!(progress.details().starts_with("A03 Test"));
    assert!(progress.handle_action(ListAction::JumpBottom));
    assert!(progress.details().starts_with("A01 Inspect"));
    assert!(progress.handle_action(ListAction::Cancel));
    assert!(!progress.focused());
    let mut next = fixture();
    next.id = "next-plan".into();
    next.version = 3;
    progress.update(next.clone(), /*observed_at*/ 1040);
    assert_eq!(progress.view.borrow().selected, None);
    progress.update(fixture(), /*observed_at*/ 1040);
    assert_eq!(progress.plan, next);
}

#[test]
fn height_pressure_drops_spacer_then_entire_overview() {
    let progress = progress(fixture());
    let mut frames = Vec::new();
    for height in 1..=7 {
        let mut buf = Buffer::empty(Rect::new(
            /*x*/ 2, /*y*/ 3, /*width*/ 40, height,
        ));
        progress.render(buf.area, &mut buf);
        frames.push(format!("height {height}\n{}", buffer_text(&buf)));
        assert_eq!(
            progress.view.borrow().rows.len(),
            usize::from(height.saturating_sub(1)).min(3)
        );
        assert_eq!(
            super::layout::VerticalLayout::new(&progress.entries(), buf.area).overview_y,
            match height {
                6 => Some(4),
                7 => Some(5),
                _ => None,
            }
        );
    }
    insta::assert_snapshot!(frames.join("\n\n"));
    for width in 0..12 {
        let mut buf = Buffer::empty(Rect::new(
            /*x*/ 5, /*y*/ 4, width, /*height*/ 0,
        ));
        progress.render(buf.area, &mut buf);
        rendered(&progress, width);
    }
    let mut completed = fixture();
    for step in &mut completed.steps {
        step.state = PlanStepState::Completed;
    }
    let completed = super::tests::progress(completed);
    assert_eq!(completed.desired_height(/*width*/ 80), 0);
    completed.toggle_focus();
    assert_eq!(completed.desired_height(/*width*/ 80), 7);
}

#[test]
fn state_markers_keep_cancellation_distinct() {
    let mut plan = fixture();
    plan.steps[0].state = PlanStepState::Cancelled;
    plan.steps[1].state = PlanStepState::Reviewing;
    plan.steps[1].elapsed_seconds = 85;
    plan.steps[1].running_since = None;
    plan.steps[2].state = PlanStepState::Blocked;
    insta::assert_snapshot!(snapshot(&progress(plan), /*width*/ 64));
}

#[test]
fn tiny_progress_and_long_times_survive_adaptive_layout() {
    let mut plan = fixture();
    plan.steps[1].running_since = None;
    plan.steps[1].elapsed_seconds = 1;
    plan.steps[1].definition.title = "验证e\u{301}执行路径".into();
    plan.steps[2].definition.estimate_seconds = 600000;
    let progress = progress(plan);
    insta::assert_snapshot!(
        [80, 46, 36, 30, 22]
            .into_iter()
            .map(|width| snapshot(&progress, width))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    let text = snapshot(&progress, /*width*/ 80);
    assert!(
        text.lines()
            .nth(2)
            .unwrap()
            .ends_with("0:01  ━─────────      1:00")
    );
    assert_eq!(label("e\u{301}验证", /*width*/ 2), "e\u{301}…");
}
