use super::*;
use crate::test_support::TempDir;
use serde_json::json;

#[test]
fn native_omlx_settings_follow_the_app_data_root() {
    let home = TempDir::new("omlx-native-discovery");
    home.write(
        ".omlx/settings.json",
        r#"{"server":{"host":"0.0.0.0","port":8001},"auth":{"api_key":"fixture-key"}}"#,
    );
    let settings = omlx_settings(Some(&home.0)).unwrap();
    assert_eq!(settings["server"]["port"], 8001);
    home.write(
        "Library/Application Support/oMLX/base-path",
        &home.0.join("moved").display().to_string(),
    );
    home.write("moved/settings.json", r#"{"server":{"port":9001}}"#);
    assert_eq!(
        omlx_settings(Some(&home.0)).unwrap()["server"]["port"],
        9001
    );
    assert!(omlx_settings(None).is_none());
    assert_eq!(local_bind("0.0.0.0"), "127.0.0.1");
    assert_eq!(local_bind("::"), "::1");
    assert_eq!(local_bind("localhost"), "localhost");
}

#[test]
fn splash_discovery_accepts_only_valid_ports() {
    assert_eq!(
        splash_port(&json!({"launchOptions":{"--port":"8100"},"serverPort":8000})),
        Some(8100)
    );
    assert_eq!(splash_port(&json!({"serverPort":8000})), Some(8000));
    for port in [json!(0), json!(65536), json!(-1), json!("bad")] {
        assert_eq!(splash_port(&json!({"serverPort":port})), None);
    }
    assert!(splash_preferences(None).is_none());
    assert!(splash_preferences(Some(&TempDir::new("no-splash-preferences").0)).is_none());
    // Reading credentials is opt-in, so ordinary discovery cannot trigger Keychain UI.
    if env::var("MLXTOP_SPLASH_KEYCHAIN").as_deref() != Ok("1") {
        assert!(splash_keychain_key().is_none());
    }
}

#[test]
fn omlx_native_discovery_respects_explicit_and_legacy_settings() {
    use crate::config::{Config, OmxConfig};
    use crate::omlx::{read_omlx_api_key, read_omlx_endpoint};
    let home = TempDir::new("omlx-discovery-precedence");
    home.write(
        ".omlx/settings.json",
        r#"{"server":{"host":"0.0.0.0","port":8001},"auth":{"api_key":"native-fixture"}}"#,
    );
    assert_eq!(
        read_omlx_endpoint(&Config::default(), Some(&home.0)),
        ("127.0.0.1".into(), 8001)
    );
    if env::var("MLXTOP_PROVIDER_API_KEY").is_err() {
        assert_eq!(
            read_omlx_api_key(Some(&home.0)).as_deref(),
            Some("native-fixture")
        );
    }
    home.write(
        ".config/omlx-coding/server.env",
        "PORT=9000\nAPI_KEY=legacy-fixture\n",
    );
    assert_eq!(
        read_omlx_endpoint(&Config::default(), Some(&home.0)).1,
        9000
    );
    if env::var("MLXTOP_PROVIDER_API_KEY").is_err() {
        assert_eq!(
            read_omlx_api_key(Some(&home.0)).as_deref(),
            Some("legacy-fixture")
        );
    }
    let config = Config {
        omx: Some(OmxConfig {
            host: Some("localhost".into()),
            port: Some(9002),
        }),
        ..Config::default()
    };
    assert_eq!(
        read_omlx_endpoint(&config, Some(&home.0)),
        ("localhost".into(), 9002)
    );
}

#[test]
fn splash_native_preferences_read_binary_or_xml_plists() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let home = TempDir::new("splash-native-preferences");
    home.write("Library/Preferences/ai.inco.splash.plist", r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>launchOptions</key><dict><key>--port</key><string>8101</string></dict></dict></plist>"#);
    let prefs = splash_preferences(Some(&home.0)).expect("read native preferences");
    assert_eq!(splash_port(&prefs), Some(8101));
}
