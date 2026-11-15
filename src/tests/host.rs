// SPDX-License-Identifier: MIT
use super::*;

#[cfg(unix)]
#[test]
fn system_host_reads_files_directories_and_commands() {
    let root = std::env::temp_dir().join(format!("mlxtop-host-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("zone0")).unwrap();
    fs::write(root.join("zone0/temp"), "41000\n").unwrap();
    let host = System;
    assert_eq!(
        host.read_file(&root.join("zone0/temp")).as_deref(),
        Some("41000\n")
    );
    assert_eq!(host.read_file(&root.join("missing")), None);
    assert_eq!(host.read_dir(&root), vec![root.join("zone0")]);
    assert!(host.read_dir(&root.join("missing")).is_empty());
    assert_eq!(host.command_u64("sh", &["-c", "echo ' 42 '"]), Some(42));
    assert_eq!(host.command_u64("sh", &["-c", "echo nope"]), None);
    assert_eq!(host.command("sh", &["-c", "exit 3"]), None);
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn platform_matches_the_build_target() {
    let expected = if cfg!(target_os = "macos") {
        Platform::MacOs
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Linux
    };
    assert_eq!(Platform::current(), expected);
}

#[test]
fn clock_reports_the_local_offset_for_request_times() {
    use crate::test_support::FakeHost;
    let host = FakeHost::default()
        .command("/bin/date +%H:%M:%S %z", "16:30:45 -0400\n")
        .command("/bin/date +%H:%M:%S %z", "08:00:00 +0530\n")
        .command("/bin/date +%H:%M:%S %z", "08:00:00 EST\n");
    let clock = |host: &FakeHost| now_clock(host, Platform::Linux);
    assert_eq!(clock(&host), ("16:30:45".into(), Some(-4 * 3_600)));
    assert_eq!(clock(&host), ("08:00:00".into(), Some(5 * 3_600 + 1_800)));
    // An unparseable zone keeps the clock and leaves request times in UTC.
    assert_eq!(clock(&host), ("08:00:00".into(), None));
    assert_eq!(clock(&FakeHost::default()), ("??:??:??".into(), None));
    // Windows has no `date`; its clock comes from the system API.
    let windows = now_clock(&FakeHost::default(), Platform::Windows);
    assert!(host.calls().len() == 3, "{:?}", host.calls());
    if cfg!(windows) {
        assert_eq!(windows.0.len(), 8);
        assert!(windows.1.is_some());
    }
    for invalid in ["", "0400", "+04", "+2460", "+0475", "+04:0"] {
        assert_eq!(parse_utc_offset(invalid), None, "{invalid}");
    }
}

#[cfg(unix)]
#[test]
fn command_output_does_not_depend_on_the_callers_locale() {
    // Commands run with numbers in the C locale and no `LC_ALL` override.
    assert_eq!(
        System
            .command("sh", &["-c", "echo ${LC_ALL-unset} $LC_NUMERIC"])
            .as_deref(),
        Some("unset C\n")
    );
    if cfg!(target_os = "macos") {
        let cpu = System
            .command("/bin/ps", &["-o", "%cpu=", "-p", "1"])
            .unwrap();
        assert!(!cpu.contains(','), "{cpu:?}");
    }
}

#[cfg(unix)]
#[test]
fn command_output_with_invalid_utf8_is_kept() {
    let output = System
        .command("/usr/bin/printf", &["a\\n\\377\\n"])
        .expect("an undecodable byte must not discard the reading");
    assert_eq!(output.lines().next(), Some("a"));
}

#[test]
fn a_missing_program_is_reported_once_until_it_runs() {
    let name = format!("mlxtop-missing-{}", std::process::id());
    assert!(first_spawn_failure(&name));
    assert!(!first_spawn_failure(&name));
    assert_eq!(System.command(&name, &[]), None);
    assert!(!first_spawn_failure(&name), "still missing: still quiet");
}

#[test]
fn system_host_reads_only_the_start_of_a_file_and_when_it_changed() {
    let root = std::env::temp_dir().join(format!("mlxtop-host-prefix-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let file = root.join("model.safetensors");
    fs::write(&file, b"\x05\0\0\0\0\0\0\0{}   weights").unwrap();
    let host = System;
    // The length and the header, never what follows them.
    assert_eq!(
        host.read_prefix(&file, 8).as_deref(),
        Some(&b"\x05\0\0\0\0\0\0\0"[..])
    );
    assert_eq!(
        host.read_prefix(&file, 13).as_deref(),
        Some(&b"\x05\0\0\0\0\0\0\0{}   "[..])
    );
    assert_eq!(
        host.read_prefix(&file, 1 << 20).map(|bytes| bytes.len()),
        Some(20)
    );
    assert!(host.modified(&file).is_some());
    // A directory and a missing file cannot be read.
    assert_eq!(host.read_prefix(&root, 8), None);
    assert_eq!(host.read_prefix(&root.join("missing"), 8), None);
    assert_eq!(host.modified(&root.join("missing")), None);
    // A host with no files reads nothing.
    struct Empty;
    impl Host for Empty {
        fn command(&self, _: &str, _: &[&str]) -> Option<String> {
            None
        }
        fn read_file(&self, _: &Path) -> Option<String> {
            None
        }
        fn read_dir(&self, _: &Path) -> Vec<PathBuf> {
            Vec::new()
        }
    }
    assert_eq!(Empty.read_prefix(&file, 8), None);
    assert_eq!(Empty.modified(&file), None);
    assert_eq!(Empty.command("true", &[]), None);
    assert_eq!(Empty.read_file(&file), None);
    assert!(Empty.read_dir(&root).is_empty());
    let _ = fs::remove_dir_all(&root);
}
