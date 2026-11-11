// SPDX-License-Identifier: MIT
//! Command-line parsing and application startup.
use crate::app::App;
use crate::collector::Collector;
use crate::config::load_doctor_config;
use crate::config::{
    config_history, config_interval, config_path, load_config, Config, HISTORY_MAX, HISTORY_MIN,
    INTERVAL_MAX, INTERVAL_MIN,
};
use crate::host::Platform;
use crate::logging::{
    diagnostics_default_hint, diagnostics_log, init_diagnostics, install_panic_hook, log_field,
};
use crate::report::{write_doctor, write_static};
use crate::terminal::{next_terminal_event, run_app, TerminalGuard};
use crate::update::update;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{stdout, IsTerminal, Write};
use std::time::Duration;
use std::{env, io, thread};
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn entry() {
    let _ = init_diagnostics();
    install_panic_hook();
    match run() {
        Ok(()) => diagnostics_log("INFO", "process_exit", "code=0"),
        Err(error) => {
            diagnostics_log(
                "ERROR",
                "process_error",
                format!("error={}", log_field(&error.to_string())),
            );
            eprintln!("mlxtop: {error}");
            std::process::exit(1);
        }
    }
}

/// What the command line asks for, after config defaults are applied.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliAction {
    /// Print this text (version or help) and exit successfully.
    Print(String),
    Doctor {
        interval: u64,
        history: usize,
    },
    Run {
        interval: u64,
        history: usize,
        once: bool,
        /// Append every sample to this file as a JSON line.
        record: Option<std::path::PathBuf>,
    },
    /// Read a recording back through today's rules and report what fires.
    Replay {
        path: std::path::PathBuf,
    },
    /// Replace this binary with the newest release (`rc`: or release candidate).
    Update {
        rc: bool,
    },
}

pub(crate) fn help_text(platform: Platform) -> String {
    format!(
        "Usage: mlxtop [refresh-seconds] [options]\n       mlxtop doctor [--interval N]\n       mlxtop update [--rc]\n       mlxtop replay FILE\n\n\
         update installs the latest release in place; --rc includes release candidates\n\
         replay reads a recording back through the detection rules and reports what fires\n\
         Options: -i, --interval N  refresh interval (default 1)\n\
         -n, --history N    chart/journal history (20–3600)\n\
         -1, --once         static report\n\
         --record FILE      append every sample to FILE (one JSON line each)\n\
         -V, --version      show version\n\
         -h, --help         show help\n\
         Config file: {}\n\
         Diagnostics: {} (override with MLXTOP_LOG_PATH)\n\n\
         Interactive keys: q quit · 1 overview · 2 top · 3 journal · tab views · arrows charts · +/- zoom · enter expand · {{/}} interval · d diagnostics · u session summary · ? help",
        if platform == Platform::Windows {
            r"%USERPROFILE%\.config\mlxtop\config.json"
        } else {
            "~/.config/mlxtop/config.json"
        },
        diagnostics_default_hint(platform)
    )
}

pub(crate) fn parse_args(
    args: &[String],
    config: &Config,
) -> Result<CliAction, Box<dyn std::error::Error>> {
    if let Some(("update", options)) = args
        .split_first()
        .map(|(first, rest)| (first.as_str(), rest))
    {
        return match options {
            [] => Ok(CliAction::Update { rc: false }),
            [flag] if flag == "--rc" => Ok(CliAction::Update { rc: true }),
            [flag] if flag == "-h" || flag == "--help" => {
                Ok(CliAction::Print(help_text(Platform::current())))
            }
            [option, ..] => Err(format!("unknown option for update: {option}").into()),
        };
    }
    if let Some(("replay", options)) = args
        .split_first()
        .map(|(first, rest)| (first.as_str(), rest))
    {
        return match options {
            [flag] if flag == "-h" || flag == "--help" => {
                Ok(CliAction::Print(help_text(Platform::current())))
            }
            [path] => Ok(CliAction::Replay { path: path.into() }),
            [] => Err("replay needs a recording: mlxtop replay FILE".into()),
            [_, option, ..] => Err(format!("unknown option for replay: {option}").into()),
        };
    }
    let (mut interval, interval_rejected) = config_interval(config);
    let (mut history, history_rejected) = config_history(config);
    if interval_rejected {
        diagnostics_log(
            "WARN",
            "config_out_of_range",
            format!(
                "field=interval value={:?} allowed={INTERVAL_MIN}..={INTERVAL_MAX} using={interval}",
                config.interval
            ),
        );
    }
    if history_rejected {
        diagnostics_log(
            "WARN",
            "config_out_of_range",
            format!(
                "field=history value={:?} allowed={HISTORY_MIN}..={HISTORY_MAX} using={history}",
                config.history
            ),
        );
    }
    let mut once = false;
    let mut doctor = false;
    let mut record = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "doctor" if i == 0 => doctor = true,
            "-i" | "--interval" => {
                i += 1;
                interval = args.get(i).ok_or("missing interval")?.parse()?;
            }
            "-n" | "--history" => {
                i += 1;
                history = args.get(i).ok_or("missing history")?.parse()?;
            }
            "-1" | "--once" => once = true,
            "--record" => {
                i += 1;
                record = Some(args.get(i).ok_or("missing recording file")?.into());
            }
            "-V" | "--version" => return Ok(CliAction::Print(format!("mlxtop {VERSION}"))),
            "-h" | "--help" => return Ok(CliAction::Print(help_text(Platform::current()))),
            value if !value.starts_with('-') && i == 0 => interval = value.parse()?,
            value => return Err(format!("unknown option: {value}").into()),
        }
        i += 1;
    }
    if !(INTERVAL_MIN..=INTERVAL_MAX).contains(&interval) {
        return Err("interval must be between 1 and 60 seconds".into());
    }
    if !(HISTORY_MIN..=HISTORY_MAX).contains(&history) {
        return Err("history must be between 20 and 3600".into());
    }
    if doctor {
        if once {
            return Err("doctor cannot be combined with --once".into());
        }
        return Ok(CliAction::Doctor { interval, history });
    }
    Ok(CliAction::Run {
        interval,
        history,
        once,
        record,
    })
}

/// Take two samples `interval` apart, so rates are measured, and report.
pub(crate) fn run_once(
    collector: &mut Collector,
    interval: Duration,
    out: &mut dyn Write,
) -> io::Result<()> {
    collector.sample();
    thread::sleep(interval);
    let sample = collector.sample();
    write_static(
        out,
        &sample,
        interval.as_secs(),
        collector.thresholds,
        collector.platform,
    )
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let config = if args.first().is_some_and(|arg| arg == "doctor") {
        load_doctor_config(&config_path())?
    } else {
        load_config()
    };
    let (interval, history, once) = match parse_args(&args, &config)? {
        CliAction::Print(text) => {
            println!("{text}");
            return Ok(());
        }
        CliAction::Doctor { interval, history } => {
            let mut collector = Collector::new(history, config);
            collector.sample();
            thread::sleep(Duration::from_secs(interval));
            let sample = collector.sample();
            if !write_doctor(&mut stdout().lock(), &sample)? {
                return Err(
                    "diagnostics found unavailable host counters or a failed runtime connection"
                        .into(),
                );
            }
            return Ok(());
        }
        CliAction::Update { rc } => return update(rc),
        CliAction::Replay { path } => {
            let thresholds = crate::config::Thresholds::from_config(&config);
            crate::recording::run(&path, thresholds, &mut stdout().lock())?;
            return Ok(());
        }
        CliAction::Run {
            interval,
            history,
            once,
            record,
        } => {
            if let Some(path) = record {
                crate::recording::start(&path)
                    .map_err(|error| format!("cannot record to {}: {error}", path.display()))?;
            }
            (interval, history, once)
        }
    };

    diagnostics_log(
        "INFO",
        "configuration",
        format!(
            "interval_seconds={interval} history_limit={history} once={once} interactive={} config_path={}",
            io::stdin().is_terminal() && io::stdout().is_terminal(),
            config_path().display()
        ),
    );

    if once {
        let mut collector = Collector::new(history, config);
        run_once(
            &mut collector,
            Duration::from_secs(interval),
            &mut stdout().lock(),
        )?;
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("run interactively, or use --once for a static report".into());
    }

    enable_raw_mode()?;
    let mut terminal_guard = TerminalGuard::new();
    let mut out = stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        crossterm::cursor::Hide
    )?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new(interval, history, config);
    let result = run_app(&mut terminal, &mut app, &mut next_terminal_event);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        crossterm::cursor::Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    terminal_guard.disarm();
    // What the session amounted to stays in the terminal after it closes.
    if app.collector.session.generating + app.collector.session.prefilling > 0.0 {
        println!("mlxtop session summary");
        for line in app.summary_lines() {
            println!("  {line}");
        }
    }
    drop(app);
    result
}
