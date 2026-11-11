// SPDX-License-Identifier: MIT
//! Physical RAM by kind: what can be reclaimed, compressed or never moved.
//! Wired memory cannot be compressed or swapped; free and file cache are
//! headroom. A marker shows the GPU working-set limit when a runtime reports it;
//! it turns yellow, with a legend reason, once wired memory exceeds the limit.
use crate::domain::Sample;
use crate::theme::{BLUE, CYAN, EDGE, YELLOW};
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Frame;

/// One segment's fill. Solid fills that differ in hue (wired,
/// app) or lightness (compressed, cache) tell kinds apart; free is the track.
struct Kind {
    color: Color,
}

const WIRED: Kind = Kind { color: BLUE };
const APP: Kind = Kind { color: CYAN };
const COMPRESSED: Kind = Kind {
    color: Color::Rgb(128, 140, 158),
};
const CACHE: Kind = Kind {
    color: Color::Rgb(84, 98, 120),
};
const FREE: Kind = Kind { color: EDGE };

/// Each kind's fill, in stacking order from the bottom: wired, app,
/// compressed, cache, free.
pub(crate) fn kind_colors() -> [Color; 5] {
    kinds().map(|kind| kind.color)
}

/// The GPU wired-memory cap: an explicit `iogpu.wired_limit_mb`, else the
/// runtime's reported Metal recommended working set.
pub(crate) fn gpu_limit(sample: &Sample) -> Option<u64> {
    sample
        .metal
        .resource_limit
        .or(sample.mlx.recommended_working_set)
        .filter(|limit| *limit > 0)
}

/// Segment sizes in bytes, clamped so they never exceed physical RAM.
pub(crate) fn segments(sample: &Sample) -> Option<[u64; 5]> {
    if !sample.vm_available || sample.total_memory == 0 {
        return None;
    }
    let mut remaining = sample.total_memory;
    let mut take = |value: u64| {
        let value = value.min(remaining);
        remaining -= value;
        value
    };
    let wired = take(sample.wired);
    let app = take(sample.anonymous);
    let compressed = take(sample.compressor);
    let cache = take(sample.file_backed);
    // Without any kind counters everything would read as free: say unknown.
    if wired + app + compressed + cache == 0 {
        return None;
    }
    Some([wired, app, compressed, cache, remaining])
}

fn kinds() -> [Kind; 5] {
    [WIRED, APP, COMPRESSED, CACHE, FREE]
}

/// The composition bar, with the GPU limit marked on it or left to a meter
/// of its own.
pub(crate) fn draw_composition(
    frame: &mut Frame,
    area: Rect,
    sample: &Sample,
    marker: bool,
) -> bool {
    let Some(sizes) = segments(sample) else {
        return false;
    };
    if area.is_empty() {
        return true;
    }
    let width = u64::from(area.width);
    let total = sample.total_memory;
    // Largest-remainder rounding keeps every column assigned exactly once.
    let mut columns = sizes.map(|size| u128::from(size) * u128::from(width) / u128::from(total));
    let assigned: u128 = columns.iter().sum();
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| {
        std::cmp::Reverse(u128::from(sizes[i]) * u128::from(width) % u128::from(total))
    });
    for &i in order.iter().take((u128::from(width) - assigned) as usize) {
        columns[i] += 1;
    }
    let mut x = area.x;
    for (kind, count) in kinds().iter().zip(columns) {
        for _ in 0..count {
            frame.buffer_mut()[(x, area.y)]
                .set_symbol(" ")
                .set_bg(kind.color);
            x += 1;
        }
    }
    // GPU working-set limit: wired model memory beyond it slows generation.
    if let Some(limit) = gpu_limit(sample).filter(|_| marker) {
        let column = (u128::from(limit.min(total)) * u128::from(width) / u128::from(total))
            .min(u128::from(width) - 1) as u16;
        let over = sample.wired > limit;
        frame.buffer_mut()[(area.x + column, area.y)]
            .set_symbol("┃")
            .set_fg(if over { YELLOW } else { Color::White });
    }
    true
}

#[cfg(test)]
#[path = "tests/memory_composition.rs"]
mod tests;
