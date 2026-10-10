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
