// SPDX-License-Identifier: MIT
#![forbid(unsafe_code)]

mod analysis;
mod app;
mod chart_navigation;
mod chart_render;
mod chart_scale;
mod cli;
mod collector;
mod completion_log;
mod config;
mod diagnosis;
mod diagnostics_view;
mod domain;
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
mod model_dashboard;
mod omlx;
mod operator_charts;
mod operator_history;
mod parsing;
mod platform;
mod process_memory;
mod processes;
mod provider_discovery;
mod providers;
mod report;
mod request_dashboard;
mod request_history;
mod runtime_diagnostics;
mod runtime_features;
mod sampler;
mod settings;
mod swap_usage;
mod terminal;
mod theme;
mod transport;
mod ui;
mod update;

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
