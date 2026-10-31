// SPDX-License-Identifier: MIT
//! Native app configuration discovery. Explicit mlxtop settings take precedence.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::{env, fs};

pub(crate) fn omlx_settings(home: Option<&Path>) -> Option<Value> {
    let home = home?;
    let base = env::var_os("OMLX_BASE_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            fs::read_to_string(home.join("Library/Application Support/oMLX/base-path"))
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(|s| PathBuf::from(s.trim()))
        })
        .unwrap_or_else(|| home.join(".omlx"));
    let base = if base.starts_with("~") {
        home.join(base.strip_prefix("~").ok()?)
    } else {
        base
    };
    serde_json::from_str(&fs::read_to_string(base.join("settings.json")).ok()?).ok()
}

pub(crate) fn local_bind(host: &str) -> String {
    match host {
        "0.0.0.0" => "127.0.0.1",
        "::" | "[::]" => "::1",
        host => host,
    }
    .to_owned()
}

pub(crate) fn splash_port(preferences: &Value) -> Option<u16> {
    preferences
        .pointer("/launchOptions/--port")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            preferences
                .get("serverPort")
                .and_then(Value::as_u64)
                .and_then(|n| n.try_into().ok())
        })
        .filter(|p| *p > 0)
}

pub(crate) fn splash_preferences(home: Option<&Path>) -> Option<Value> {
    let path = home?.join("Library/Preferences/ai.inco.splash.plist");
    if !path.is_file() {
        return None;
    }
    let output = crate::host::command_text(
        "/usr/bin/plutil",
        &["-convert", "json", "-o", "-", "--", path.to_str()?],
    )?;
    serde_json::from_str(&output).ok()
}

/// Read only on explicit opt-in: macOS can ask the user to grant Keychain access.
/// The key never appears in command arguments, diagnostics or configuration files.
pub(crate) fn splash_keychain_key() -> Option<String> {
    if !cfg!(target_os = "macos") || env::var("MLXTOP_SPLASH_KEYCHAIN").as_deref() != Ok("1") {
        return None;
    }
    crate::host::command_text(
        "/usr/bin/security",
        &[
            "find-generic-password",
            "-s",
            "Splash",
            "-a",
            "API key",
            "-w",
        ],
    )
    .map(|s| s.trim_end_matches(['\r', '\n']).to_owned())
    .filter(|s| !s.is_empty() && !s.chars().any(char::is_control))
}

#[cfg(test)]
#[path = "tests/provider_discovery.rs"]
mod tests;
