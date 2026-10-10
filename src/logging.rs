// SPDX-License-Identifier: MIT
//! Local diagnostic logging, rotation and panic reporting.
use crate::cli::VERSION;
use crate::host::Platform;
use std::any::Any;
use std::backtrace::Backtrace;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;
use std::{env, fs, panic};
pub(crate) const DIAGNOSTICS_LOG_ENV: &str = "MLXTOP_LOG_PATH";
pub(crate) const DIAGNOSTICS_MAX_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();

pub(crate) struct Diagnostics {
    pub(crate) path: PathBuf,
    pub(crate) file: Mutex<File>,
    pub(crate) max_bytes: u64,
}

impl Diagnostics {
    pub(crate) fn open(path: PathBuf, max_bytes: u64) -> Option<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).ok()?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        Some(Self {
            path,
            file: Mutex::new(file),
            max_bytes,
        })
    }

    pub(crate) fn log(&self, level: &str, event: &str, details: &str) {
        let details = details.replace(['\r', '\n'], "\\n");
        if let Ok(mut file) = self.file.lock() {
            if file
                .metadata()
                .map(|metadata| metadata.len() >= self.max_bytes)
                .unwrap_or(false)
            {
                let rotated = self.path.with_extension("log.1");
                let _ = file.flush();
                if fs::rename(&self.path, rotated).is_ok() {
                    if let Ok(replacement) = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&self.path)
                    {
                        *file = replacement;
                    }
                } else if let Ok(truncated) = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&self.path)
                {
                    // An append-only handle cannot be shortened on Windows;
                    // a fresh truncating handle works on every platform.
                    *file = truncated;
                }
            }
            let _ = writeln!(
                file,
                "ts_ms={} level={} event={} {}",
                diagnostics_timestamp_ms(),
                level,
                event,
                details
            );
            let _ = file.flush();
        }
    }
}

pub(crate) fn init_diagnostics() -> Option<&'static Diagnostics> {
    install_diagnostics(diagnostics_path()?)
}

/// Open the process-wide log at `path` unless one is already open, then
/// record the session start in whichever log is active.
pub(crate) fn install_diagnostics(path: PathBuf) -> Option<&'static Diagnostics> {
    if DIAGNOSTICS.get().is_none() {
        let _ = DIAGNOSTICS.set(Diagnostics::open(path, DIAGNOSTICS_MAX_BYTES)?);
    }
    let diagnostics = DIAGNOSTICS.get()?;
    diagnostics.log(
        "INFO",
        "session_start",
        &format!(
            "version={} pid={} log_path={}",
            VERSION,
            std::process::id(),
            log_field(&diagnostics.path.display().to_string())
        ),
    );
    Some(diagnostics)
}

pub(crate) fn diagnostics_path() -> Option<PathBuf> {
    let platform = Platform::current();
    diagnostics_path_from(
        env::var(DIAGNOSTICS_LOG_ENV).ok(),
        // Windows keeps per-user state in %LOCALAPPDATA%.
        env::var(if platform == Platform::Windows {
            "LOCALAPPDATA"
        } else {
            "XDG_STATE_HOME"
        })
        .ok(),
        crate::config::home_dir(),
        platform,
    )
}

pub(crate) fn diagnostics_path_from(
    log_path: Option<String>,
    state_home: Option<String>,
    home: Option<PathBuf>,
    platform: Platform,
) -> Option<PathBuf> {
    if let Some(path) = log_path {
        let path = path.trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    if platform == Platform::Windows {
        return state_home
            .map(|dir| dir.trim().to_owned())
            .filter(|dir| !dir.is_empty())
            .map(|dir| PathBuf::from(dir).join("mlxtop").join("mlxtop.log"))
            .or_else(|| home.map(|home| home.join("AppData/Local/mlxtop/mlxtop.log")))
            .or_else(|| Some(PathBuf::from("mlxtop.log")));
    }
    if platform == Platform::Linux {
        if let Some(state_home) = state_home {
            let state_home = state_home.trim();
            if !state_home.is_empty() {
                return Some(PathBuf::from(state_home).join("mlxtop/mlxtop.log"));
            }
        }
        return home
            .map(|home| home.join(".local/state/mlxtop/mlxtop.log"))
            .or_else(|| Some(PathBuf::from("mlxtop.log")));
    }
    home.map(|home| home.join("Library/Logs/mlxtop/mlxtop.log"))
        .or_else(|| Some(PathBuf::from("mlxtop.log")))
}

pub(crate) fn diagnostics_default_hint(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "~/.local/state/mlxtop/mlxtop.log",
        Platform::MacOs => "~/Library/Logs/mlxtop/mlxtop.log",
        Platform::Windows => r"%LOCALAPPDATA%\mlxtop\mlxtop.log",
    }
}

pub(crate) fn diagnostics_log(level: &str, event: &str, details: impl AsRef<str>) {
    if let Some(diagnostics) = DIAGNOSTICS.get() {
        diagnostics.log(level, event, details.as_ref());
    }
}

pub(crate) fn diagnostics_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) fn log_field(value: &str) -> String {
    let mut field = String::new();
    for character in value.chars().take(160) {
        if character.is_ascii_alphanumeric()
            || matches!(character, '-' | '_' | '.' | '/' | ':' | '%' | '@')
        {
            field.push(character);
        } else if character.is_whitespace() {
            field.push('_');
        } else {
            field.push('?');
        }
    }
    field
}

pub(crate) fn log_optional_f64(value: Option<f64>) -> String {
    value
        .filter(|value| value.is_finite())
        .map(|value| format!("{value:.3}"))
        .unwrap_or_else(|| "na".into())
}

pub(crate) fn log_optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "na".into())
}

pub(crate) fn log_optional_u8(value: Option<u8>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "na".into())
}

pub(crate) fn panic_payload(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".into()
    }
}

pub(crate) fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    // Windows reports `src\x.rs`; keep one form in the log.
                    location.file().replace('\\', "/"),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".into());
        let message = panic_payload(info.payload());
        let backtrace = Backtrace::force_capture();
        diagnostics_log(
            "ERROR",
            "panic",
            format!(
                "message={} location={} backtrace={backtrace}",
                log_field(&message),
                log_field(&location),
            ),
        );
        previous(info);
    }));
}
