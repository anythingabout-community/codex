//! Presentation-only grouping preserves every durable step and its evidence.
use super::*;

impl PlanProgress {
    pub(super) fn entries(&self) -> Vec<Entry> {
        let now = self.timestamp();
        let mut entries = Vec::new();
        for stage in &self.plan.stages {
            let steps: Vec<_> = self
                .plan
                .steps
                .iter()
                .enumerate()
                .filter(|(_, step)| step.definition.stage_id == stage.id)
                .collect();
            let old = steps.last().is_some_and(|(index, _)| {
                *index < self.plan.steps.len().saturating_sub(/*rhs*/ 4)
            });
            if steps.is_empty()
                || (old
                    && steps
                        .iter()
                        .all(|(_, step)| step.state == PlanStepState::Completed))
            {
                entries.push(Entry {
                    key: format!("stage:{}", stage.id),
                    id: stage.id.clone(),
                    title: if steps.is_empty() {
                        stage.title.clone()
                    } else {
                        format!("{} ({} tasks)", stage.title, steps.len())
                    },
                    seconds: if steps.is_empty() {
                        stage.estimate_seconds
                    } else {
                        steps.iter().map(|(_, step)| step.elapsed_at(now)).sum()
                    },
                    elapsed: steps.iter().map(|(_, step)| step.elapsed_at(now)).sum(),
                    state: if steps.is_empty() {
                        PlanStepState::Pending
                    } else {
                        PlanStepState::Completed
                    },
                    step: None,
                });
            } else {
                entries.extend(steps.into_iter().map(|(index, step)| Entry {
                    key: format!("step:{}", step.definition.id),
                    id: step.definition.id.clone(),
                    title: step.definition.title.clone(),
                    seconds: step.definition.estimate_seconds,
                    elapsed: step.elapsed_at(now),
                    state: step.state,
                    step: Some(index),
                }));
            }
        }
        entries
    }
}
