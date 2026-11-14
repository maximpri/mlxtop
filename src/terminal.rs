// SPDX-License-Identifier: MIT
//! Terminal lifecycle, restoration and the interactive event loop.
use crate::app::App;
use crate::logging::{diagnostics_log, log_field, panic_payload};
use crossterm::event::{DisableMouseCapture, Event};
use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
use crossterm::{event, execute};
use ratatui::backend::Backend;
use ratatui::Terminal;
use std::io::stdout;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
use std::{io, panic};
pub(crate) struct TerminalGuard {
    pub(crate) active: bool,
}

impl TerminalGuard {
    pub(crate) fn new() -> Self {
        Self { active: true }
    }

    pub(crate) fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let mut out = stdout();
            let _ = execute!(
                out,
                crossterm::cursor::Show,
                DisableMouseCapture,
                LeaveAlternateScreen
            );
            crate::logging::set_tui_active(false);
            crate::logging::report_deferred_panic();
        }
    }
}

/// Wait up to `timeout` for one terminal event.
pub(crate) fn next_terminal_event(timeout: Duration) -> io::Result<Option<Event>> {
    match event::poll(timeout) {
        Ok(false) => Ok(None),
        Ok(true) => event::read().map(Some).inspect_err(|error| {
            diagnostics_log(
                "ERROR",
                "input_read_error",
                format!("error={}", log_field(&error.to_string())),
            );
        }),
        Err(error) => {
            diagnostics_log(
                "ERROR",
                "input_poll_error",
                format!("error={}", log_field(&error.to_string())),
            );
            Err(error)
        }
    }
}

pub(crate) fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    next_event: &mut dyn FnMut(Duration) -> io::Result<Option<Event>>,
) -> Result<(), Box<dyn std::error::Error>> {
    diagnostics_log("INFO", "tui_start", "interactive_session_started");
    loop {
        if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| app.tick())) {
            diagnostics_log(
                "ERROR",
                "tui_tick_panic",
                format!(
                    "tab={} status={} message={}",
                    app.tab,
                    log_field(&app.collector.current.llm_status),
                    log_field(&panic_payload(payload.as_ref()))
                ),
            );
            panic::resume_unwind(payload);
        }
        let terminal_size = terminal
            .size()
            .ok()
            .map(|size| format!("{}x{}", size.width, size.height))
            .unwrap_or_else(|| "unknown".into());
        let app_for_draw = &mut *app;
        let draw_result = panic::catch_unwind(AssertUnwindSafe(|| {
            terminal.draw(move |frame| app_for_draw.draw(frame))
        }));
        let draw_result = match draw_result {
            Ok(result) => result,
            Err(payload) => {
                diagnostics_log(
                    "ERROR",
                    "tui_draw_panic",
                    format!(
                        "tab={} terminal={} status={} message={}",
                        app.tab,
                        terminal_size,
                        log_field(&app.collector.current.llm_status),
                        log_field(&panic_payload(payload.as_ref()))
                    ),
                );
                panic::resume_unwind(payload);
            }
        };
        if let Err(error) = draw_result {
            diagnostics_log(
                "ERROR",
                "terminal_draw_error",
                format!("error={}", log_field(&error.to_string())),
            );
            return Err(error.into());
        }
        if app.quit {
            break;
        }
        match next_event(Duration::from_millis(100))? {
            Some(Event::Key(key)) => app.handle_key(key),
            Some(Event::Mouse(mouse)) => app.handle_mouse(mouse),
            Some(_) | None => {}
        }
    }
    diagnostics_log("INFO", "tui_stop", "interactive_session_stopped");
    Ok(())
}
