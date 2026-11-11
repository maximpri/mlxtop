// SPDX-License-Identifier: MIT
use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const GIB: u64 = 1024 * 1024 * 1024;

fn sample() -> Sample {
    Sample {
        vm_available: true,
        total_memory: 32 * GIB,
        wired: 4 * GIB,
        anonymous: 16 * GIB,
        compressor: 4 * GIB,
        file_backed: 4 * GIB,
        ..Sample::default()
    }
}

/// The bar's cells: symbols, foregrounds and backgrounds.
fn bar(sample: &Sample, width: u16, marker: bool) -> (String, Vec<Color>, Vec<Color>, bool) {
    let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
    let mut drawn = false;
    terminal
        .draw(|frame| drawn = draw_composition(frame, frame.area(), sample, marker))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (
        (0..width).map(|x| buffer[(x, 0)].symbol()).collect(),
        (0..width).map(|x| buffer[(x, 0)].fg).collect(),
        (0..width).map(|x| buffer[(x, 0)].bg).collect(),
        drawn,
    )
}

#[test]
fn composition_partitions_ram_and_never_exceeds_it() {
    assert_eq!(
        segments(&sample()),
        Some([4 * GIB, 16 * GIB, 4 * GIB, 4 * GIB, 4 * GIB])
    );
    // Overlapping counters are clamped instead of overflowing physical RAM.
    let mut oversized = sample();
    oversized.anonymous = 40 * GIB;
    assert_eq!(segments(&oversized), Some([4 * GIB, 28 * GIB, 0, 0, 0]));
    assert_eq!(segments(&Sample::default()), None);
    let counters_missing = Sample {
        vm_available: true,
        total_memory: 32 * GIB,
        ..Sample::default()
    };
    assert_eq!(
        segments(&counters_missing),
        None,
        "never report all RAM as free"
    );

    let (symbols, _, backgrounds, drawn) = bar(&sample(), 32, true);
    assert!(drawn);
    assert_eq!(symbols.chars().count(), 32, "every column is assigned once");
    // Solid fills, no textures: wired, app, compressed, cache, free track.
    assert!(symbols.chars().all(|c| c == ' '), "{symbols}");
    let fills = kind_colors();
    assert_eq!(fills[0], BLUE);
    assert_eq!(fills[1], CYAN);
    for (range, fill) in [(0..4, 0), (4..20, 1), (20..24, 2), (24..28, 3), (28..32, 4)] {
        assert!(
            backgrounds[range.clone()]
                .iter()
                .all(|bg| *bg == fills[fill]),
            "{range:?}: {backgrounds:?}"
        );
    }
    // Compressed and cache differ in lightness, not just in name.
    assert_ne!(fills[2], fills[3]);
    // Missing counters draw nothing and say so to the caller.
    let (_, _, _, drawn) = bar(&Sample::default(), 32, true);
    assert!(!drawn);
}

#[test]
fn gpu_limit_is_marked_on_the_bar_unless_a_meter_shows_it() {
    let mut sample = sample();
    sample.mlx.recommended_working_set = Some(24 * GIB);
    let (symbols, colors, _, _) = bar(&sample, 32, true);
    assert_eq!(symbols.chars().nth(24), Some('┃'));
    assert_eq!(colors[24], Color::White);
    sample.wired = 26 * GIB;
    let (_, colors, _, _) = bar(&sample, 32, true);
    assert_eq!(colors[24], YELLOW, "worth inspecting, not critical");
    // The headroom panel gives the limit a meter of its own.
    let (symbols, _, _, _) = bar(&sample, 32, false);
    assert!(!symbols.contains('┃'), "{symbols}");
    // An explicit iogpu.wired_limit_mb wins over the runtime's estimate.
    sample.metal.resource_limit = Some(28 * GIB);
    assert_eq!(gpu_limit(&sample), Some(28 * GIB));
    sample.metal.resource_limit = Some(0);
    sample.mlx.recommended_working_set = None;
    assert_eq!(gpu_limit(&sample), None, "a zero limit is no limit");
}
