// SPDX-License-Identifier: MIT
use super::*;
use crate::config::{config_interval, load_config_from, Thresholds};
use crate::sampler::{Sampler, SamplerCommand};
use crate::test_support::TempDir;
use crate::tests::{render_app, test_app};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

fn press(app: &mut App, codes: &[KeyCode]) {
    for code in codes {
        app.handle_key(KeyEvent::new(*code, KeyModifiers::NONE));
    }
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, &[KeyCode::Char(c)]);
    }
}

/// An app on the Settings tab, editing `config.json` in a scratch directory.
fn settings_app(dir: &TempDir) -> (App, Receiver<SamplerCommand>) {
    let (commands, command_receiver) = mpsc::channel();
    let (_views_sender, views) = mpsc::channel();
    let mut app = test_app(0);
    app.sampler = Sampler {
        commands,
        views,
        handle: None,
    };
    app.settings = Settings::new(dir.0.join("mlxtop/config.json"));
    press(&mut app, &[KeyCode::Char('4')]);
    assert_eq!(app.tab, 3);
    (app, command_receiver)
}

fn select(app: &mut App, key: &str) {
    app.settings.selected = FIELDS.iter().position(|f| f.key == key).unwrap();
}

fn file(dir: &TempDir) -> Value {
    serde_json::from_str(&fs::read_to_string(dir.0.join("mlxtop/config.json")).unwrap()).unwrap()
}

#[test]
fn every_config_key_has_a_field_and_every_field_round_trips() {
    // Each field's key must be one the config reads: a typo would save a key nothing uses.
    for field in FIELDS {
        let value = match field.kind {
            Kind::Number { default, .. } | Kind::Rate { default } => Value::from(default),
            Kind::Toggle { default } => Value::from(!default),
            Kind::Host => Value::from("10.0.0.2"),
            Kind::Port => Value::from(9000),
        };
        let mut doc = Map::new();
        match field.key.split_once('.') {
            Some((object, key)) => doc.insert(object.into(), serde_json::json!({ key: value })),
            None => doc.insert(field.key.into(), value.clone()),
        };
        let config: Config = serde_json::from_value(Value::Object(doc)).unwrap();
        assert_ne!(config, Config::default(), "{} reaches Config", field.key);
        assert_eq!(
            field.parse(&field.edit_text(&value)),
            Ok(value),
            "{}",
            field.key
        );
    }
    // And every Config key is offered: adding a config field without a setting fails here.
    let every: Value = serde_json::from_str(crate::tests::CONFIG_WITH_EVERY_FIELD_SET).unwrap();
    for (key, value) in every.as_object().unwrap() {
        let keys: Vec<String> = match value {
            Value::Object(inner) => inner.keys().map(|k| format!("{key}.{k}")).collect(),
            _ => vec![key.clone()],
        };
        for key in keys {
            assert!(FIELDS.iter().any(|f| f.key == key), "{key} has no setting");
        }
    }
}

#[test]
fn values_are_checked_before_saving() {
    let field = |key: &str| FIELDS.iter().find(|f| f.key == key).unwrap();
    assert!(field("interval").parse("0").is_err());
    assert!(field("interval").parse("61").is_err());
    assert_eq!(field("interval").parse(" 5 "), Ok(Value::from(5)));
    assert!(field("history").parse("10").is_err());
    assert!(field("gpu_warn_load").parse("101").is_err());
    assert!(field("omx.port").parse("0").is_err());
    assert!(field("omx.host").parse("my host").is_err());
    assert!(field("swap_warn_rate").parse("-1").is_err());
    assert_eq!(
        field("swap_warn_rate").parse("0.5"),
        Ok(Value::from(MIB / 2))
    );
    assert_eq!(
        field("swap_warn_rate").display(&Value::from(16 * MIB)),
        "16 MiB/s"
    );
}

#[test]
fn editing_saves_the_file_applies_now_and_keeps_unknown_keys() {
    let dir = TempDir::new("settings-edit");
    dir.write(
        "mlxtop/config.json",
        r#"{"history": 400, "custom": "kept"}"#,
    );
    let (mut app, commands) = settings_app(&dir);
    press(&mut app, &[KeyCode::Char('4')]);
    assert_eq!(
        app.settings.value(app.settings.field()),
        None,
        "interval is unset"
    );

    select(&mut app, "interval");
    press(&mut app, &[KeyCode::Enter]);
    type_text(&mut app, "5");
    press(&mut app, &[KeyCode::Enter]);
    assert_eq!(app.settings.editing, None);
    assert!(matches!(app.settings.status, Some(Status::Saved(_))));
    assert_eq!(
        app.interval,
        Duration::from_secs(5),
        "applied without a restart"
    );
    let saved = file(&dir);
    assert_eq!(saved["interval"], 5);
    assert_eq!(saved["history"], 400);
    assert_eq!(
        saved["custom"], "kept",
        "keys Settings does not know survive"
    );
    let config = load_config_from(&dir.0.join("mlxtop/config.json"));
    assert_eq!(
        config_interval(&config),
        (5, false),
        "the next start reads it too"
    );
    let sent: Vec<_> = commands.try_iter().collect();
    assert!(sent
        .iter()
        .any(|c| matches!(c, SamplerCommand::SetInterval(d) if *d == Duration::from_secs(5))));
    assert!(sent
        .iter()
        .any(|c| matches!(c, SamplerCommand::SetConfig(_))));

    // A rate is typed in MiB/s and stored in bytes; thresholds change at once.
    select(&mut app, "swap_warn_rate");
    press(&mut app, &[KeyCode::Enter]);
    type_text(&mut app, "2");
    press(&mut app, &[KeyCode::Enter]);
    assert_eq!(file(&dir)["swap_warn_rate"], 2 * MIB);
    assert_eq!(app.thresholds.swap_warn_rate, 2 * MIB);

    // A nested key goes into its object, and resetting the last one removes the object.
    select(&mut app, "omx.port");
    press(&mut app, &[KeyCode::Enter]);
    type_text(&mut app, "9000");
    press(&mut app, &[KeyCode::Enter]);
    assert_eq!(file(&dir)["omx"]["port"], 9000);
    press(&mut app, &[KeyCode::Char('x')]);
    assert!(file(&dir).get("omx").is_none());

    // x resets to the default by removing the key.
    select(&mut app, "interval");
    press(&mut app, &[KeyCode::Char('x')]);
    assert!(file(&dir).get("interval").is_none());
    assert_eq!(app.interval, Duration::from_secs(1));
}

#[test]
fn bad_input_is_refused_and_esc_cancels_without_quitting() {
    let dir = TempDir::new("settings-invalid");
    let (mut app, _commands) = settings_app(&dir);
    select(&mut app, "gpu_warn_load");
    press(&mut app, &[KeyCode::Enter]);
    type_text(&mut app, "150");
    press(&mut app, &[KeyCode::Enter]);
    assert!(matches!(app.settings.status, Some(Status::Error(_))));
    assert!(
        app.settings.editing.is_some(),
        "the edit stays open to correct"
    );
    assert!(
        !dir.0.join("mlxtop/config.json").exists(),
        "nothing written"
    );
    // Digits and q are text while editing, not view switches or quit.
    type_text(&mut app, "q2");
    assert_eq!(app.settings.editing.as_deref(), Some("150q2"));
    assert_eq!(app.tab, 3);
    press(&mut app, &[KeyCode::Esc]);
    assert!(app.settings.editing.is_none() && !app.quit_prompt && !app.quit);
    press(&mut app, &[KeyCode::Esc]);
    assert!(
        app.quit_prompt,
        "with no edit open, Esc asks to quit as elsewhere"
    );
}

#[test]
fn turning_alarms_off_clears_a_raised_alarm() {
    let dir = TempDir::new("settings-alarms");
    let (mut app, _commands) = settings_app(&dir);
    app.alert = Some(crate::app::ActiveAlert {
        state: "HEAVY PAGING".into(),
        summary: String::new(),
        time: String::new(),
    });
    select(&mut app, "alarms");
    press(&mut app, &[KeyCode::Enter]);
    assert_eq!(file(&dir)["alarms"], false);
    assert!(!app.alarms && app.alert.is_none());
    press(&mut app, &[KeyCode::Char(' ')]);
    assert_eq!(file(&dir)["alarms"], true);
    assert!(app.alarms);
}

#[test]
fn a_file_that_does_not_parse_is_never_overwritten() {
    let dir = TempDir::new("settings-broken");
    let broken = r#"{"interval": 2,,}"#;
    dir.write("mlxtop/config.json", broken);
    let (mut app, _commands) = settings_app(&dir);
    assert!(app.settings.broken.is_some());
    select(&mut app, "alarms");
    press(&mut app, &[KeyCode::Enter]);
    assert!(matches!(app.settings.status, Some(Status::Error(_))));
    assert_eq!(
        fs::read_to_string(dir.0.join("mlxtop/config.json")).unwrap(),
        broken
    );
    assert_eq!(app.thresholds, Thresholds::default());
}

#[test]
fn the_view_lists_every_setting_and_shows_the_file() {
    let dir = TempDir::new("settings-render");
    dir.write("mlxtop/config.json", r#"{"gpu_warn_load": 60}"#);
    let (app, _commands) = settings_app(&dir);
    let screen = render_app(&app, 120, 40);
    assert!(screen.contains("4 Settings"));
    for heading in ["General", "oMLX server", "Thresholds"] {
        assert!(screen.contains(heading), "{heading}");
    }
    assert!(screen.contains("60%"), "a set value");
    assert!(
        screen.contains("default 75%"),
        "and the default it replaces"
    );
    assert!(screen.contains("config.json"));
    // A short terminal scrolls the list to keep the selection visible.
    let mut app = app;
    app.settings.selected = FIELDS.len() - 1;
    let screen = render_app(&app, 80, 24);
    assert!(screen.contains("Compression clears below"));
}

#[cfg(unix)]
#[test]
fn saving_keeps_a_symlinked_config_linked_and_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new("settings-symlink");
    let target = dir.write("dotfiles/mlxtop.json", "{\"interval\": 2}\n");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.0.join("config.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let settings = Settings::new(link.clone());
    let mut doc = serde_json::Map::new();
    doc.insert("interval".into(), serde_json::json!(3));
    settings.write(&doc).unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(fs::read_to_string(&target)
        .unwrap()
        .contains("\"interval\": 3"));
    let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}
