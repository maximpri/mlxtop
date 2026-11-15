// SPDX-License-Identifier: MIT
//! The QA judge on the states the chip's bandwidth adds to the runtime card:
//! the widest figure beside a long chip name, and a chip whose bin mlxtop
//! cannot tell.
use super::qa::{judge, Case, SIZES};
use super::*;

fn with_chip(name: &str, cores: Option<u16>) -> App {
    let mut app = test_app(0);
    populate_dashboard_fixture(&mut app);
    app.collector.current.metal.device_name = Some(name.into());
    app.collector.current.metal.gpu_cores = cores;
    app
}

#[test]
fn the_chips_bandwidth_passes_the_qa_judge_at_every_size() {
    let cases = [
        Case {
            name: "chip-large",
            impact: false,
            app: with_chip("Apple M5 Ultra", Some(80)),
        },
        Case {
            name: "chip-unknown-bin",
            impact: false,
            app: with_chip("Apple M4 Max", None),
        },
    ];
    let mut failures = Vec::new();
    for case in &cases {
        for (width, height) in SIZES {
            for problem in judge(case, width, height) {
                failures.push(format!("{} at {width}×{height}: {problem}", case.name));
            }
            let screen = render_app(&case.app, width, height);
            let shown = screen.matches("GB/s").count();
            // Once per screen at most, never cut, and gone first at 80 columns.
            let expected = match case.name {
                "chip-large" if width > 80 => 1,
                _ => 0,
            };
            if shown != expected || (shown > 0 && !screen.contains("· 1200 GB/s ·")) {
                failures.push(format!(
                    "{} at {width}×{height}: bandwidth shown {shown} times\n{screen}",
                    case.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "QA judge:\n{}", failures.join("\n"));
}
