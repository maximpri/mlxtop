// SPDX-License-Identifier: MIT
//! Persistent configuration, precedence, bounds and severity thresholds.
use crate::domain::{Tone, MIB};
use crate::logging::diagnostics_log;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::{env, fs};
pub(crate) const MEMORY_WARN_LOAD: u64 = 70;
pub(crate) const MEMORY_CRITICAL_LOAD: u64 = 85;
pub(crate) const GPU_WARN_LOAD: u64 = 75;
pub(crate) const GPU_CRITICAL_LOAD: u64 = 90;
pub(crate) const SWAP_WARN_RATE: u64 = MIB;
pub(crate) const SWAP_CRITICAL_RATE: u64 = 16 * MIB;
pub(crate) const COMPRESSION_WARN_RATE: u64 = 64 * MIB;
pub(crate) const SWAP_WARN_EXIT: u64 = 2 * MIB;
/** Churn rate that turns "paging active" on; `swap_warn_exit` turns it off. */
pub(crate) const PAGING_ACTIVE_ENTER_RATE: u64 = 4 * MIB;
/** GPU load that turns "GPU busy" on; `gpu_warn_exit` turns it off. */
pub(crate) const GPU_BUSY_ENTER_LOAD: u64 = 80;
pub(crate) const COMPRESSION_WARN_EXIT: u64 = 32 * MIB;
pub(crate) const GPU_WARN_EXIT: u64 = 70;
pub(crate) const DEFAULT_OMLX_HOST: &str = "127.0.0.1";
pub(crate) const DEFAULT_OMLX_PORT: u16 = 8080;

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct Config {
    pub(crate) interval: Option<u64>,
    pub(crate) history: Option<usize>,
    pub(crate) omx: Option<OmxConfig>,
    pub(crate) memory_warn_load: Option<u64>,
    pub(crate) memory_critical_load: Option<u64>,
    pub(crate) gpu_warn_load: Option<u64>,
    pub(crate) gpu_critical_load: Option<u64>,
    pub(crate) swap_warn_rate: Option<u64>,
    pub(crate) swap_critical_rate: Option<u64>,
    pub(crate) compression_warn_rate: Option<u64>,
    pub(crate) swap_warn_exit: Option<u64>,
    pub(crate) compression_warn_exit: Option<u64>,
    pub(crate) gpu_warn_exit: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub(crate) struct OmxConfig {
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
}

pub(crate) fn load_config() -> Config {
    load_config_from(&config_path())
}

pub(crate) fn load_config_from(config_path: &Path) -> Config {
    match fs::read_to_string(config_path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(config) => {
                diagnostics_log(
                    "INFO",
                    "config_loaded",
                    format!("path={}", config_path.display()),
                );
                config
            }
            Err(e) => {
                diagnostics_log(
                    "WARN",
                    "config_parse_error",
                    format!("path={} error={}", config_path.display(), e),
                );
                Config::default()
            }
        },
        Err(_) => Config::default(),
    }
}

pub(crate) const INTERVAL_MIN: u64 = 1;
pub(crate) const INTERVAL_MAX: u64 = 60;
pub(crate) const INTERVAL_DEFAULT: u64 = 1;
pub(crate) const HISTORY_MIN: usize = 20;
pub(crate) const HISTORY_MAX: usize = 3600;
pub(crate) const HISTORY_DEFAULT: usize = 300;

/**
 * Resolve `interval` from the config file.
 *
 * An out-of-range entry falls back to the default and reports `true` so the
 * caller can log it: a typo in a file the user edits by hand should not stop
 * mlxtop from starting. An out-of-range CLI argument is still a hard error,
 * because the user typed it just now and can see the message.
 */
pub(crate) fn config_interval(config: &Config) -> (u64, bool) {
    match config.interval {
        Some(value) if (INTERVAL_MIN..=INTERVAL_MAX).contains(&value) => (value, false),
        Some(_) => (INTERVAL_DEFAULT, true),
        None => (INTERVAL_DEFAULT, false),
    }
}

/**
 * Resolve `history` from the config file, with the same fallback rule as
 * [`config_interval`].
 */
pub(crate) fn config_history(config: &Config) -> (usize, bool) {
    match config.history {
        Some(value) if (HISTORY_MIN..=HISTORY_MAX).contains(&value) => (value, false),
        Some(_) => (HISTORY_DEFAULT, true),
        None => (HISTORY_DEFAULT, false),
    }
}

pub(crate) fn config_path() -> PathBuf {
    config_path_in(home_dir())
}

/// `HOME`, or `USERPROFILE` on Windows where `HOME` is usually unset.
pub(crate) fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn config_path_in(home: Option<PathBuf>) -> PathBuf {
    if let Some(home) = home {
        home.join(".config/mlxtop/config.json")
    } else {
        PathBuf::from("config.json")
    }
}

/**
 * Effective severity thresholds for one run.
 *
 * Defaults mirror the built-in constants; every field can be replaced by the
 * matching key in `~/.config/mlxtop/config.json`. Values are resolved once at
 * startup and then handed to the severity, alert and correlation code, so a
 * configured value changes what the user actually sees.
 */
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Thresholds {
    pub(crate) memory_warn_load: u64,
    pub(crate) memory_critical_load: u64,
    pub(crate) gpu_warn_load: u64,
    pub(crate) gpu_critical_load: u64,
    pub(crate) gpu_warn_exit: u64,
    pub(crate) swap_warn_rate: u64,
    pub(crate) swap_critical_rate: u64,
    pub(crate) swap_warn_exit: u64,
    pub(crate) compression_warn_rate: u64,
    pub(crate) compression_warn_exit: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            memory_warn_load: MEMORY_WARN_LOAD,
            memory_critical_load: MEMORY_CRITICAL_LOAD,
            gpu_warn_load: GPU_WARN_LOAD,
            gpu_critical_load: GPU_CRITICAL_LOAD,
            gpu_warn_exit: GPU_WARN_EXIT,
            swap_warn_rate: SWAP_WARN_RATE,
            swap_critical_rate: SWAP_CRITICAL_RATE,
            swap_warn_exit: SWAP_WARN_EXIT,
            compression_warn_rate: COMPRESSION_WARN_RATE,
            compression_warn_exit: COMPRESSION_WARN_EXIT,
        }
    }
}

impl Thresholds {
    pub(crate) fn from_config(config: &Config) -> Self {
        let defaults = Self::default();
        Self {
            memory_warn_load: config.memory_warn_load.unwrap_or(defaults.memory_warn_load),
            memory_critical_load: config
                .memory_critical_load
                .unwrap_or(defaults.memory_critical_load),
            gpu_warn_load: config.gpu_warn_load.unwrap_or(defaults.gpu_warn_load),
            gpu_critical_load: config
                .gpu_critical_load
                .unwrap_or(defaults.gpu_critical_load),
            gpu_warn_exit: config.gpu_warn_exit.unwrap_or(defaults.gpu_warn_exit),
            swap_warn_rate: config.swap_warn_rate.unwrap_or(defaults.swap_warn_rate),
            swap_critical_rate: config
                .swap_critical_rate
                .unwrap_or(defaults.swap_critical_rate),
            swap_warn_exit: config.swap_warn_exit.unwrap_or(defaults.swap_warn_exit),
            compression_warn_rate: config
                .compression_warn_rate
                .unwrap_or(defaults.compression_warn_rate),
            compression_warn_exit: config
                .compression_warn_exit
                .unwrap_or(defaults.compression_warn_exit),
        }
        .normalized()
    }

    /**
     * Keep the bands usable no matter what the file says: percentages stay
     * within 0..=100, a critical level never sits below its warning level,
     * and a hysteresis exit never sits above the level that turns the state
     * on. Bad input is clamped rather than rejected so a single stray value
     * cannot silently remove a severity band.
     */
    pub(crate) fn normalized(mut self) -> Self {
        self.memory_warn_load = self.memory_warn_load.min(100);
        self.memory_critical_load = self.memory_critical_load.clamp(self.memory_warn_load, 100);
        self.gpu_warn_load = self.gpu_warn_load.min(100);
        self.gpu_critical_load = self.gpu_critical_load.clamp(self.gpu_warn_load, 100);
        self.gpu_warn_exit = self.gpu_warn_exit.min(100);
        self.swap_critical_rate = self.swap_critical_rate.max(self.swap_warn_rate);
        self.compression_warn_exit = self.compression_warn_exit.min(self.compression_warn_rate);
        self
    }
}

/** Shared warn/critical banding used by every load-style indicator. */
pub(crate) fn load_tone(value: u64, warn: u64, critical: u64) -> Tone {
    if value >= critical {
        Tone::Red
    } else if value >= warn {
        Tone::Yellow
    } else {
        Tone::Green
    }
}

pub(crate) fn load_doctor_config(path: &Path) -> Result<Config, &'static str> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(_) => return Err("cannot read mlxtop configuration file"),
    };
    let config: Config = serde_json::from_str(&text)
        .map_err(|_| "invalid mlxtop configuration JSON or field type")?;
    if config_interval(&config).1 || config_history(&config).1 {
        return Err("configuration interval/history is out of range");
    }
    if config.omx.as_ref().and_then(|omx| omx.port) == Some(0) {
        return Err("oMLX port must be between 1 and 65535");
    }
    Ok(config)
}
