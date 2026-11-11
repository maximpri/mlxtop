use crate::test_support::*;
// SPDX-License-Identifier: MIT
use super::*;
use ratatui::backend::TestBackend;

fn render(sample: &Sample, width: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
    terminal
        .draw(|frame| draw(frame, frame.area(), sample))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

#[test]
fn capacity_bar_keeps_units_and_occupancy_at_compact_and_wide_sizes() {
    let sample = Sample {
        swap_available: true,
        swap_used: 1024 * MIB,
        swap_total: 2 * 1024 * MIB,
        ..Sample::default()
    };
    for width in [30, 80, 180] {
        let buffer = render(&sample, width);
        assert!(text(&buffer).contains("1.0/2.0 GiB 50%"));
        let filled = buffer.content.iter().filter(|cell| cell.bg == CYAN).count();
        let empty = buffer.content.iter().filter(|cell| cell.bg == EDGE).count();
        assert!(filled >= 3);
        assert!(filled.abs_diff(empty) <= 1);
        assert!(buffer
            .content
            .iter()
            .all(|cell| cell.fg != RED && cell.fg != YELLOW));
    }
}

#[test]
fn unavailable_unallocated_and_unused_swap_are_distinct() {
    let mut sample = Sample::default();
    let missing = render(&sample, 40);
    assert!(text(&missing).contains("swap —"));
    assert!(!text(&missing).contains("0%"));
    sample.swap_available = true;
    let unallocated = render(&sample, 40);
    assert!(text(&unallocated).contains("swap 0 B used · no swap file yet"));
    assert!(unallocated.content.iter().all(|cell| cell.bg != CYAN));
    sample.swap_total = 2 * 1024 * MIB;
    let unused = render(&sample, 40);
    assert!(text(&unused).contains("0.0/2.0 GiB 0%"));
    assert!(unused.content.iter().any(|cell| cell.bg == EDGE));
    assert!(unused.content.iter().all(|cell| cell.bg != CYAN));
}

#[test]
fn narrow_rows_keep_the_exact_label_and_tiny_values_use_bytes() {
    let sample = Sample {
        swap_available: true,
        swap_used: 512,
        swap_total: 1000,
        ..Sample::default()
    };
    let narrow = text(&render(&sample, 16));
    assert!(narrow.starts_with("swap 512/1000 B"), "{narrow}");
    assert!(render(&sample, 16)
        .content
        .iter()
        .all(|cell| cell.bg != CYAN));
    assert_eq!(capacity(3 * 1024, 4 * 1024), "3.0/4.0 KiB");
    // An empty area draws nothing rather than panicking.
    let mut terminal = Terminal::new(TestBackend::new(4, 1)).unwrap();
    terminal
        .draw(|frame| draw(frame, Rect::new(0, 0, 0, 0), &sample))
        .unwrap();
    assert_eq!(text(terminal.backend().buffer()).trim(), "");
}
