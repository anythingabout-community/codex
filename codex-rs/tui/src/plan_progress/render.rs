//! Task-list styling with independently measured row metadata and a continuous overview.
use super::layout::BAR_WIDTH;
use super::layout::MAX_ROWS;
use super::layout::RowLayout;
use super::layout::Timing;
use super::layout::VerticalLayout;
use super::layout::segments;
use super::*;
use ratatui::style::Styled;
use ratatui::text::Line;

impl Entry {
    fn style(&self) -> Style {
        match self.state {
            PlanStepState::Running | PlanStepState::Reviewing => Style::default().cyan(),
            PlanStepState::Blocked => Style::default().red(),
            PlanStepState::Completed | PlanStepState::Cancelled | PlanStepState::Pending => {
                Style::default().dim()
            }
        }
    }
}

fn bar(buf: &mut Buffer, area: Rect, entry: &Entry) {
    let fraction = if entry.state == PlanStepState::Completed {
        1.0
    } else {
        (entry.elapsed as f64 / entry.seconds.max(/*other*/ 1) as f64)
            .clamp(/*min*/ 0.0, /*max*/ 1.0)
    };
    let filled = ((fraction * f64::from(area.width)).floor() as u16)
        .max(u16::from(fraction > 0.0))
        .min(area.width);
    for x in 0..area.width {
        buf[(area.x + x, area.y)]
            .set_symbol(if x < filled { "━" } else { "─" })
            .set_style(if x < filled {
                entry.style()
            } else {
                Style::default().dim()
            });
    }
}

impl Renderable for PlanProgress {
    fn desired_height(&self, width: u16) -> u16 {
        let complete = self.plan.steps.iter().all(|step| {
            matches!(
                step.state,
                PlanStepState::Completed | PlanStepState::Cancelled
            )
        }) && self.plan.stages.iter().all(|stage| {
            self.plan
                .steps
                .iter()
                .any(|step| step.definition.stage_id == stage.id)
        });
        if complete && !self.focused() {
            return 0;
        }
        let entries = self.entries();
        let overview = !segments(&entries, width.saturating_sub(/*rhs*/ 4)).is_empty();
        1 + entries.len().min(MAX_ROWS) as u16 + if overview { 3 } else { 0 }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let entries = self.entries();
        let layout = VerticalLayout::new(&entries, area);
        let completed = self
            .plan
            .steps
            .iter()
            .filter(|step| step.state == PlanStepState::Completed)
            .count();
        buf.set_line(
            area.x,
            area.y,
            &Line::from(vec![
                "• ".dim(),
                "Task List".bold(),
                format!("  {completed}/{}", self.plan.steps.len()).dim(),
            ]),
            area.width,
        );
        let first = entries.len() - layout.rows;
        let visible = &entries[first..];
        let rows = RowLayout::new(visible, area.width);
        let mut view = self.view.borrow_mut();
        view.rows = visible.iter().map(|entry| entry.key.clone()).collect();
        if !view.focused
            || !view
                .selected
                .as_ref()
                .is_some_and(|key| view.rows.contains(key))
        {
            view.selected = visible
                .iter()
                .rfind(|entry| entry.state == PlanStepState::Running)
                .or_else(|| visible.first())
                .map(|entry| entry.key.clone());
        }
        for (row, entry) in visible.iter().enumerate() {
            let y = area.y + 1 + row as u16;
            let selected = view.focused && view.selected.as_ref() == Some(&entry.key);
            let branch = if selected {
                "  › "
            } else if row == 0 {
                "  └ "
            } else {
                "    "
            };
            let status = match entry.state {
                PlanStepState::Completed => "✔ ",
                PlanStepState::Cancelled => "× ",
                PlanStepState::Running | PlanStepState::Pending => "□ ",
                PlanStepState::Reviewing => "◷ ",
                PlanStepState::Blocked => "! ",
            };
            buf.set_line(
                area.x,
                y,
                &Line::from(vec![
                    branch.set_style(if selected {
                        Style::default().cyan()
                    } else {
                        Style::default().dim()
                    }),
                    status.set_style(entry.style()),
                ]),
                area.width,
            );
            let available = area.width.saturating_sub(/*rhs*/ 6);
            if available > 0 {
                let id = label(&entry.id, available);
                buf.set_stringn(area.x + 6, y, id, usize::from(available), entry.style());
                let prefix = 6 + entry.id.width() + 1;
                let title_width = usize::from(area.width.saturating_sub(rows.trailing))
                    .saturating_sub(prefix) as u16;
                if title_width > 0 {
                    let style = match entry.state {
                        PlanStepState::Completed => entry.style().crossed_out(),
                        PlanStepState::Running => entry.style().bold(),
                        PlanStepState::Cancelled
                        | PlanStepState::Reviewing
                        | PlanStepState::Blocked
                        | PlanStepState::Pending => entry.style(),
                    };
                    buf.set_stringn(
                        area.x + prefix as u16,
                        y,
                        label(&entry.title, title_width),
                        usize::from(title_width),
                        if selected { style.underlined() } else { style },
                    );
                }
            }
            let elapsed = duration(entry.elapsed);
            let estimate = duration(entry.seconds);
            let right = area.right();
            if rows.timing != Timing::Hidden {
                buf.set_stringn(
                    right - estimate.width() as u16,
                    y,
                    &estimate,
                    estimate.width(),
                    Style::default().dim(),
                );
            }
            match rows.timing {
                Timing::Bar => {
                    let bar_x = right - rows.estimate - 2 - BAR_WIDTH;
                    buf.set_stringn(
                        bar_x - 2 - elapsed.width() as u16,
                        y,
                        &elapsed,
                        elapsed.width(),
                        Style::default().dim(),
                    );
                    bar(buf, Rect::new(bar_x, y, BAR_WIDTH, /*height*/ 1), entry);
                }
                Timing::Arrow => {
                    let arrow_x = right - rows.estimate - 2;
                    buf.set_stringn(
                        arrow_x - 1 - elapsed.width() as u16,
                        y,
                        &elapsed,
                        elapsed.width(),
                        Style::default().dim(),
                    );
                    buf.set_stringn(
                        arrow_x,
                        y,
                        "→",
                        /*max_width*/ 1,
                        Style::default().dim(),
                    );
                }
                Timing::Estimate | Timing::Hidden => {}
            }
        }
        if let Some(overview_y) = layout.overview_y {
            let y = area.y + overview_y;
            let mut x = area.x + 4;
            for (position, (index, width)) in
                segments(&entries, area.width.saturating_sub(/*rhs*/ 4))
                    .into_iter()
                    .enumerate()
            {
                let entry = &entries[index];
                buf.set_stringn(x, y, &entry.id, usize::from(width), entry.style());
                bar(buf, Rect::new(x, y + 1, width, /*height*/ 1), entry);
                if position > 0 {
                    let junction = if buf[(x, y + 1)].symbol() == "━" {
                        "┿"
                    } else {
                        "┼"
                    };
                    buf[(x, y + 1)]
                        .set_symbol(junction)
                        .set_style(Style::default().dim());
                }
                x += width;
            }
        }
        if self
            .plan
            .steps
            .iter()
            .any(|step| step.state == PlanStepState::Running)
        {
            self.frames
                .schedule_frame_in(Duration::from_secs(/*secs*/ 1));
        }
    }
}
