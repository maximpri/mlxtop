// SPDX-License-Identifier: MIT
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{fs, thread};

use crate::logging::{diagnostics_log, log_field};
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) fn command_text(program: &str, args: &[&str]) -> Option<String> {
    let command = || {
        format!(
            "program={} args={}",
            log_field(program),
            args.iter()
                .map(|arg| log_field(arg))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    let mut child = match Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            diagnostics_log(
                "WARN",
                "command_spawn_failed",
                format!("{} error={}", command(), log_field(&error.to_string())),
            );
            return None;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        diagnostics_log("WARN", "command_stdout_unavailable", command());
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let command_context = command();
    let reader = thread::spawn(move || {
        let mut output = String::new();
        match stdout.read_to_string(&mut output) {
            Ok(_) => Some(output),
            Err(error) => {
                diagnostics_log(
                    "WARN",
                    "command_read_failed",
                    format!(
                        "{} error={}",
                        command_context,
                        log_field(&error.to_string())
                    ),
                );
                None
            }
        }
    });
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match reader.join() {
                    Ok(Some(output)) => output,
                    Ok(None) => return None,
                    Err(_) => {
                        diagnostics_log("WARN", "command_reader_panicked", command());
                        return None;
                    }
                };
                if !status.success() {
                    diagnostics_log(
                        "WARN",
                        "command_nonzero_exit",
                        format!("{} code={:?}", command(), status.code()),
                    );
                    return None;
                }
                return Some(output);
            }
            Err(error) => {
                diagnostics_log(
                    "WARN",
                    "command_wait_failed",
                    format!("{} error={}", command(), log_field(&error.to_string())),
                );
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            Ok(None) if Instant::now() >= deadline => {
                diagnostics_log("WARN", "command_timeout", command());
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

/// Local wall-clock time and the local offset from UTC (`+HHMM`) in seconds.
pub(crate) fn now_clock(host: &dyn Host, platform: Platform) -> (String, Option<i32>) {
    if platform == Platform::Windows {
        return windows_clock();
    }
    let Some(value) = host.command("/bin/date", &["+%H:%M:%S %z"]) else {
        return ("??:??:??".into(), None);
    };
    let mut fields = value.split_whitespace();
    let clock = fields.next().unwrap_or("??:??:??").to_string();
    (clock, fields.next().and_then(parse_utc_offset))
}

#[cfg(windows)]
fn windows_clock() -> (String, Option<i32>) {
    let now = chrono::Local::now();
    (
        now.format("%H:%M:%S").to_string(),
        Some(now.offset().local_minus_utc()),
    )
}

#[cfg(not(windows))]
fn windows_clock() -> (String, Option<i32>) {
    ("??:??:??".into(), None)
}

fn parse_utc_offset(text: &str) -> Option<i32> {
    let (sign, digits) = match text.as_bytes().first()? {
        b'+' => (1, &text[1..]),
        b'-' => (-1, &text[1..]),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    (hours <= 23 && minutes <= 59).then_some(sign * (hours * 3_600 + minutes * 60))
}
// Operating-system inputs read by the collectors.
//
// Collectors reach commands and files only through [`Host`], so the parsing
// and classification paths for both platforms can be driven by recorded
// fixtures in tests. [`System`] is the production implementation; it adds no
// behavior beyond the bounded command runner and plain file reads.

pub(crate) trait Host: Send {
    /// Standard output of a successful command, or `None`.
    fn command(&self, program: &str, args: &[&str]) -> Option<String>;
    /// Whole text file, or `None` when it cannot be read.
    fn read_file(&self, path: &Path) -> Option<String>;
    /// Entries of a directory; empty when it cannot be listed.
    fn read_dir(&self, path: &Path) -> Vec<PathBuf>;
    /// Memory and processes from the Windows APIs; `None` on other systems,
    /// where the same readings come from commands and files.
    fn windows(&self) -> Option<WindowsReading> {
        None
    }

    fn command_u64(&self, program: &str, args: &[&str]) -> Option<u64> {
        self.command(program, args)?.trim().parse().ok()
    }
}

/// One Windows sample. Windows has no `ps` or `/proc`, so the host returns
/// structured values and the collectors stay free of platform APIs.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WindowsReading {
    pub total: u64,
    pub available: u64,
    /// Commit charge above physical memory, which the page file backs.
    pub swap_total: u64,
    pub swap_used: u64,
    pub processes: Vec<crate::processes::ProcessRow>,
}

/// The production host for this build target.
pub(crate) fn system() -> Box<dyn Host> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsSystem::default())
    }
    #[cfg(not(windows))]
    {
        Box::new(System)
    }
}

pub(crate) struct System;

impl Host for System {
    fn command(&self, program: &str, args: &[&str]) -> Option<String> {
        command_text(program, args)
    }

    fn read_file(&self, path: &Path) -> Option<String> {
        fs::read_to_string(path).ok()
    }

    fn read_dir(&self, path: &Path) -> Vec<PathBuf> {
        fs::read_dir(path)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::processes::ProcessRow;
    use std::sync::Mutex;
    use sysinfo::{MemoryRefreshKind, ProcessRefreshKind, ProcessesToUpdate, UpdateKind};

    /// Keeps one `sysinfo::System` so CPU use is measured between samples.
    #[derive(Default)]
    pub(crate) struct WindowsSystem(Mutex<sysinfo::System>);

    impl Host for WindowsSystem {
        fn command(&self, program: &str, args: &[&str]) -> Option<String> {
            command_text(program, args)
        }

        fn read_file(&self, path: &Path) -> Option<String> {
            System.read_file(path)
        }

        fn read_dir(&self, path: &Path) -> Vec<PathBuf> {
            System.read_dir(path)
        }

        fn windows(&self) -> Option<WindowsReading> {
            let mut system = self.0.lock().ok()?;
            system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
            system.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing()
                    .with_memory()
                    .with_cpu()
                    .with_exe(UpdateKind::OnlyIfNotSet)
                    .with_cmd(UpdateKind::OnlyIfNotSet),
            );
            let total = system.total_memory();
            let processes = system
                .processes()
                .iter()
                .map(|(pid, process)| {
                    let name = process.name().to_string_lossy().into_owned();
                    let command = process
                        .cmd()
                        .iter()
                        .map(|part| part.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join(" ");
                    ProcessRow {
                        pid: pid.as_u32(),
                        rss: process.memory(),
                        cpu: f64::from(process.cpu_usage()),
                        memory_percent: (total > 0)
                            .then(|| process.memory() as f64 * 100.0 / total as f64),
                        state: "?".into(),
                        pageins: None,
                        command: if command.is_empty() {
                            process
                                .exe()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| name.clone())
                        } else {
                            command
                        },
                        name,
                    }
                })
                .collect();
            Some(WindowsReading {
                total,
                available: system.available_memory(),
                swap_total: system.total_swap(),
                swap_used: system.used_swap(),
                processes,
            })
        }
    }
}

/// Which collector family samples the host. Chosen once from the build target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

#[cfg(test)]
#[path = "tests/host.rs"]
mod tests;
