//! Shared measurements for task rows and the two aligned overview lines.
use super::*;

pub(super) const MAX_ROWS: usize = 5;
pub(super) const BAR_WIDTH: u16 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Timing {
    Bar,
    Arrow,
    Estimate,
    Hidden,
}

pub(super) struct RowLayout {
    pub timing: Timing,
    pub estimate: u16,
    /// Includes the two-column gap separating the title from the metadata.
    pub trailing: u16,
}

impl RowLayout {
    pub fn new(entries: &[Entry], width: u16) -> Self {
        let elapsed = entries
            .iter()
            .map(|entry| duration(entry.elapsed).width())
            .max()
            .unwrap_or(/*default*/ 0) as u16;
        let estimate = entries
            .iter()
            .map(|entry| duration(entry.seconds).width())
            .max()
            .unwrap_or(/*default*/ 0) as u16;
        let names = entries
            .iter()
            .map(|entry| 6 + entry.id.width() + 1 + entry.title.width().min(/*other*/ 12))
            .max()
            .unwrap_or(/*default*/ 0);
        let (timing, trailing) = [
            (Timing::Bar, elapsed + estimate + BAR_WIDTH + 6),
            (Timing::Arrow, elapsed + estimate + 5),
            (Timing::Estimate, estimate + 2),
            (Timing::Hidden, 0),
        ]
        .into_iter()
        .find(|(_, trailing)| names + usize::from(*trailing) <= usize::from(width))
        .unwrap_or((Timing::Hidden, 0));
        Self {
            timing,
            estimate,
            trailing,
        }
    }
}

/// Only complete suffix segments are visible. Both overview rows use these widths.
pub(super) fn segments(entries: &[Entry], width: u16) -> Vec<(usize, u16)> {
    let mut start = entries.len();
    let mut used = 0usize;
    while start > 0 {
        let minimum = (entries[start - 1].id.width() + 1).max(/*other*/ 4);
        if used + minimum > usize::from(width) {
            break;
        }
        used += minimum;
        start -= 1;
    }
    let weights: Vec<_> = entries[start..]
        .iter()
        .map(|entry| {
            let seconds = match entry.state {
                PlanStepState::Completed | PlanStepState::Cancelled => entry.elapsed,
                PlanStepState::Pending
                | PlanStepState::Running
                | PlanStepState::Reviewing
                | PlanStepState::Blocked => entry.seconds,
            };
            (seconds.max(/*other*/ 0) as f64).ln_1p()
        })
        .collect();
    let total = weights.iter().sum::<f64>().max(/*other*/ 1.0);
    let extra = f64::from(width) - used as f64;
    let mut result: Vec<_> = entries
        .iter()
        .enumerate()
        .skip(start)
        .zip(weights)
        .map(|((index, entry), weight)| {
            (
                index,
                (entry.id.width() + 1).max(/*other*/ 4) as u16
                    + (extra * weight / total).floor() as u16,
            )
        })
        .collect();
    let remainder = width - result.iter().map(|(_, width)| *width).sum::<u16>();
    if let Some((_, size)) = result.last_mut() {
        *size += remainder;
    }
    result
}

pub(super) struct VerticalLayout {
    pub rows: usize,
    pub overview_y: Option<u16>,
}

impl VerticalLayout {
    pub fn new(entries: &[Entry], area: Rect) -> Self {
        let rows = entries.len().min(MAX_ROWS);
        let overview = !segments(entries, area.width.saturating_sub(/*rhs*/ 4)).is_empty();
        let overview_y = if overview && usize::from(area.height) >= rows + 3 {
            Some(1 + rows as u16 + u16::from(usize::from(area.height) >= rows + 4))
        } else {
            None
        };
        Self {
            rows: rows.min(usize::from(area.height.saturating_sub(/*rhs*/ 1))),
            overview_y,
        }
    }
}
