// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]

mod analysis;
mod anomaly;
mod app;
mod cache_impact;
mod chart_navigation;
mod chart_panels;
mod chart_render;
mod chart_scale;
mod cli;
mod collector;
mod completion_log;
mod config;
mod decode_tracker;
mod diagnosis;
mod diagnostics_view;
mod domain;
mod drop_marks;
mod formatting;
mod gpu;
mod gpu_amd;
mod gpu_dashboard;
mod gpu_findings;
mod history;
mod host;
mod json;
mod logging;
mod memory_composition;
mod memory_headroom;
mod model_dashboard;
mod omlx;
mod operator_charts;
mod operator_history;
mod parsing;
mod platform;
mod process_memory;
mod processes;
mod providers;
mod recording;
mod report;
mod request_dashboard;
mod request_history;
mod runtime_diagnostics;
mod sampler;
mod session;
mod settings;
mod swap_usage;
mod terminal;
mod theme;
mod transport;
mod ui;
mod update;
mod view_chrome;
mod view_journal;
mod view_processes;

fn main() {
    cli::entry();
}

#[cfg(test)]
#[path = "tests/app.rs"]
mod app_tests;
#[cfg(test)]
#[path = "tests/collector.rs"]
mod collector_tests;
#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;
#[cfg(test)]
#[path = "tests/main.rs"]
mod tests;
