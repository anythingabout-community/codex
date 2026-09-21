use super::*;
use crate::keymap::ListAction;
use codex_protocol::continuous_planning::ContinuousPlanning;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

impl BottomPane {
    pub(crate) fn update_timed_plan(
        &mut self,
        plan: ContinuousPlanning,
        coordinator: String,
        observed_at: i64,
    ) {
        if let Some(progress) = &mut self.plan_progress {
            progress.update(plan, observed_at);
        } else {
            self.plan_progress = Some(crate::plan_progress::PlanProgress::new(
                plan,
                coordinator,
                observed_at,
                self.frame_requester.clone(),
            ));
        }
    }

    pub(crate) fn has_timed_plan(&self) -> bool {
        self.plan_progress
            .as_ref()
            .is_some_and(|progress| progress.desired_height(/*width*/ 1) > 0)
    }

    pub(crate) fn has_active_timed_plan(&self) -> bool {
        self.plan_progress.as_ref().is_some_and(|progress| {
            progress.plan.steps.iter().any(|step| {
                matches!(
                    step.state,
                    codex_protocol::continuous_planning::PlanStepState::Running
                        | codex_protocol::continuous_planning::PlanStepState::Reviewing,
                )
            })
        })
    }

    pub(crate) fn handle_plan_key(&mut self, key: KeyEvent) -> bool {
        if !self.no_modal_or_popup_active() || self.questions.is_some() {
            return false;
        }
        if crate::key_hint::ctrl(KeyCode::Char('c')).is_press(key) {
            return false;
        }
        let Some(progress) = &self.plan_progress else {
            return false;
        };
        if self.keymap.app.focus_plan.is_pressed(key) {
            progress.toggle_focus();
            return true;
        }
        let action = self.keymap.list.action_for(key);
        if progress.focused()
            && action == Some(ListAction::Accept)
            && key.kind == KeyEventKind::Press
        {
            let text = progress.details();
            self.push_view(Box::new(PlanDetail {
                text,
                done: false,
                scroll: 0,
            }));
            return true;
        }
        action.is_some_and(|action| progress.handle_action(action))
    }
}

struct PlanDetail {
    text: String,
    done: bool,
    scroll: u16,
}

impl BottomPaneView for PlanDetail {
    fn handle_key_event(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.done = true,
            KeyCode::Down | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
    }
    fn is_complete(&self) -> bool {
        self.done
    }
    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.done = true;
        CancellationEvent::Handled
    }
}

impl Renderable for PlanDetail {
    fn desired_height(&self, _width: u16) -> u16 {
        10
    }
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let wrapped: Vec<_> = self
            .text
            .lines()
            .flat_map(|line| textwrap::wrap(line, usize::from(area.width.max(1))))
            .collect();
        Paragraph::new(
            wrapped
                .iter()
                .map(std::convert::AsRef::as_ref)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .scroll((self.scroll, 0))
        .render(area, buf);
    }
}
