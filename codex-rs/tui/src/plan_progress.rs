//! A compact live task list with a logarithmic overview and per-task progress.

use std::cell::RefCell;
use std::time::Duration;
use std::time::Instant;

use chrono::Local;
use chrono::TimeZone;
use codex_protocol::continuous_planning::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::keymap::ListAction;
use crate::render::renderable::Renderable;
use crate::tui::FrameRequester;

pub(crate) struct PlanProgress {
    pub plan: ContinuousPlanning,
    observed_at: i64,
    received: Instant,
    view: RefCell<Viewport>,
    frames: FrameRequester,
}

#[derive(Default)]
struct Viewport {
    focused: bool,
    selected: Option<String>,
    rows: Vec<String>,
}

struct Entry {
    key: String,
    id: String,
    title: String,
    seconds: i64,
    elapsed: i64,
    state: PlanStepState,
    step: Option<usize>,
}

impl PlanProgress {
    pub fn new(
        plan: ContinuousPlanning,
        _coordinator: String,
        observed_at: i64,
        frames: FrameRequester,
    ) -> Self {
        Self {
            plan,
            observed_at,
            received: Instant::now(),
            view: RefCell::new(Viewport::default()),
            frames,
        }
    }

    pub fn update(&mut self, plan: ContinuousPlanning, observed_at: i64) {
        // Revisions remain monotonic when a conversation starts its next task.
        if plan.version < self.plan.version {
            return;
        }
        if plan.id != self.plan.id {
            *self.view.borrow_mut() = Viewport::default();
        }
        self.plan = plan;
        self.observed_at = observed_at;
        self.received = Instant::now();
    }

    pub fn toggle_focus(&self) {
        let mut view = self.view.borrow_mut();
        view.focused = !view.focused;
    }

    pub fn handle_action(&self, action: ListAction) -> bool {
        let mut view = self.view.borrow_mut();
        if !view.focused {
            return false;
        }
        let position = view
            .rows
            .iter()
            .position(|key| Some(key) == view.selected.as_ref())
            .unwrap_or(/*default*/ 0);
        let next = match action {
            ListAction::MoveUp => position.saturating_sub(/*rhs*/ 1),
            ListAction::MoveDown => (position + 1).min(view.rows.len().saturating_sub(/*rhs*/ 1)),
            ListAction::JumpTop => 0,
            ListAction::JumpBottom => view.rows.len().saturating_sub(/*rhs*/ 1),
            ListAction::Cancel => {
                view.focused = false;
                return true;
            }
            ListAction::MoveLeft
            | ListAction::MoveRight
            | ListAction::PageUp
            | ListAction::PageDown
            | ListAction::Accept => return false,
        };
        if let Some(key) = view.rows.get(next).cloned() {
            view.selected = Some(key);
        }
        true
    }

    pub fn details(&self) -> String {
        let selected = self.view.borrow().selected.clone();
        let entries = self.entries();
        let Some(entry) = entries
            .iter()
            .find(|entry| Some(&entry.key) == selected.as_ref())
        else {
            return self.plan.reason.clone();
        };
        let Some(selected) = entry.step else {
            let stage_id = &entry.id;
            let details = self
                .plan
                .steps
                .iter()
                .filter(|step| &step.definition.stage_id == stage_id)
                .map(|step| {
                    format!(
                        "{} {}\n{}\n{}",
                        step.definition.id,
                        step.definition.title,
                        step.definition.acceptance,
                        step.evidence.join("\n")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            return format!(
                "{} {}\n\n{} → {}\n\n{details}",
                entry.id,
                entry.title,
                duration(entry.elapsed),
                duration(entry.seconds)
            );
        };
        self.plan.steps.get(selected).map(|step| {
            let now = self.timestamp();
            let elapsed = step.elapsed_at(now);
            let forecast = self.plan.forecast_at(now);
            let predicted = forecast.steps[selected];
            format!("{} {}\n\n{}\n\nInitial: {}s · Current: {}s · Actual: {}s · Δ {:+}s\nStarted: {}\nSubmitted: {}\nCompleted: {}\nStep ETA: {} · Task ETA: {}\n\n{}\n\n{}",
                step.definition.id, step.definition.title, step.definition.acceptance, step.initial_estimate_seconds, step.definition.estimate_seconds,
                elapsed, elapsed - step.initial_estimate_seconds, timestamp(step.started_at), timestamp(step.submitted_at), timestamp(step.completed_at),
                timestamp(Some(step.completed_at.unwrap_or(predicted))), timestamp(Some(forecast.completed_at)), step.evidence.join("\n"), self.plan.reason)
        }).unwrap_or_else(|| self.plan.reason.clone())
    }

    pub fn focused(&self) -> bool {
        self.view.borrow().focused
    }

    fn timestamp(&self) -> i64 {
        self.observed_at
            .saturating_add(self.received.elapsed().as_secs() as i64)
    }
}

mod render;

mod entries;
mod layout;

fn label(title: &str, width: u16) -> String {
    let available = usize::from(width);
    let clipped = title.width() > available;
    let name_width = available.saturating_sub(usize::from(clipped));
    let mut name = String::new();
    let mut used = 0;
    for grapheme in title.graphemes(true) {
        if used + grapheme.width() > name_width {
            break;
        }
        name.push_str(grapheme);
        used += grapheme.width();
    }
    if clipped && available > 0 {
        name.push('…');
    }
    name
}

fn duration(seconds: i64) -> String {
    let seconds = seconds.max(/*other*/ 0);
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn timestamp(value: Option<i64>) -> String {
    value
        .and_then(|value| Local.timestamp_opt(value, /*nsecs*/ 0).single())
        .map_or_else(
            || "—".to_string(),
            |value| value.format("%m-%d %H:%M:%S").to_string(),
        )
}

#[cfg(test)]
#[path = "plan_progress_tests.rs"]
mod tests;
