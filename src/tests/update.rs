// SPDX-License-Identifier: MIT
use super::*;
use crate::test_support::TempDir;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn release(tag: &str, prerelease: bool, draft: bool) -> Release {
    Release {
        tag_name: tag.into(),
        draft,
        prerelease,
        assets: Vec::new(),
    }
}

#[test]
fn versions_parse_strictly_and_finals_sort_after_their_candidates() {
    assert_eq!(version("3.0.0").to_string(), "3.0.0");
    assert_eq!(version("3.1.0-rc.12").to_string(), "3.1.0-rc.12");
    assert!(version("3.1.0-rc.1").is_rc());
    assert!(!version("3.1.0").is_rc());
    for invalid in [
        "",
        "3",
        "3.0",
        "3.0.0.0",
        "v3.0.0",
        "3.0.x",
        "3..0",
        "+3.0.0",
        "3.0.0-rc.0",
        "3.0.0-rc.01",
        "3.0.0-rc.",
        "3.0.0-rc.+1",
        "3.0.0-beta.1",
        "3.0.0-rc.18446744073709551615",
    ] {
        assert_eq!(Version::parse(invalid), None, "{invalid}");
    }
    let mut ordered = [
        "3.1.0",
        "3.1.0-rc.10",
        "2.9.9",
        "3.1.0-rc.2",
        "3.0.10",
        "3.0.9",
    ]
    .map(version)
    .to_vec();
    ordered.sort();
    assert_eq!(
        ordered.iter().map(ToString::to_string).collect::<Vec<_>>(),
        [
            "2.9.9",
            "3.0.9",
            "3.0.10",
            "3.1.0-rc.2",
            "3.1.0-rc.10",
            "3.1.0"
        ]
    );
}

#[test]
fn release_selection_skips_drafts_and_takes_candidates_only_with_rc() {
    let releases = || {
        vec![
            release("v3.0.0", false, false),
            release("v3.1.0-rc.2", true, false),
            release("v3.1.0-rc.3", true, true),
            release("v2.1.1", false, false),
            release("nightly", true, false),
        ]
    };
    let tag = |rc| select_release(releases(), rc).map(|(_, release)| release.tag_name);
    assert_eq!(tag(false).as_deref(), Some("v3.0.0"));
    assert_eq!(tag(true).as_deref(), Some("v3.1.0-rc.2"));
    // Once the final ships, the RC channel follows it.
    let mut with_final = releases();
    with_final.push(release("v3.1.0", false, false));
    assert_eq!(
        select_release(with_final, true).map(|(version, _)| version),
        Some(version("3.1.0"))
    );
    // An RC tag never reaches the stable channel, even if left unflagged, and
    // a final flagged as a prerelease stays out of it too.
    let mislabeled = vec![
        release("v3.2.0-rc.1", false, false),
        release("v3.1.5", true, false),
    ];
    assert!(select_release(mislabeled, false).is_none());
    assert!(select_release(Vec::new(), true).is_none());
}

#[test]
fn checksums_come_from_sha256sums_lines_for_the_exact_file() {
    let digest = "AB".repeat(32);
    let sums = format!(
        "{}  mlxtop-3.0.0-aarch64-apple-darwin.dmg\nshort  mlxtop.tar.gz\n{digest} *mlxtop.tar.gz\n",
        "0".repeat(64)
    );
    assert_eq!(
        published_sha256(&sums, "mlxtop.tar.gz"),
        Some("ab".repeat(32))
    );
    assert_eq!(published_sha256(&sums, "mlxtop.zip"), None);
    assert_eq!(published_sha256("zz  mlxtop.zip", "mlxtop.zip"), None);
    assert_eq!(
        published_sha256(&format!("{}  x", "g".repeat(64)), "x"),
        None
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn package_managed_installs_are_left_to_their_package_manager() {
    for path in [
        "/opt/homebrew/Cellar/mlxtop/3.0.0/bin/mlxtop",
        "/home/linuxbrew/.linuxbrew/bin/mlxtop",
    ] {
        assert!(managed_install(Path::new(path))
            .unwrap()
            .contains("brew upgrade"));
    }
    assert!(managed_install(Path::new(
        "/Users/me/.local/share/mlxtop/rc/releases/3.1.0-rc.1-abc/mlxtop"
    ))
    .unwrap()
    .contains("rc.py"));
    for path in [
        "/Users/me/.local/bin/mlxtop",
        "/usr/local/bin/mlxtop",
        "/Users/me/.cargo/bin/mlxtop",
    ] {
        assert_eq!(managed_install(Path::new(path)), None, "{path}");
    }
    assert_eq!(
        archive_name(version("3.0.0"), "x86_64-pc-windows-msvc"),
        "mlxtop-3.0.0-x86_64-pc-windows-msvc.zip"
    );
    assert_eq!(
        archive_name(version("3.1.0-rc.1"), "aarch64-apple-darwin"),
        "mlxtop-3.1.0-rc.1-aarch64-apple-darwin.tar.gz"
    );
}

#[test]
fn notices_follow_the_installers_layout() {
    let dir = TempDir::new("update-notices");
    let v = version("3.1.0");
    let bin = dir.0.join("prefix/bin/mlxtop");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    assert_eq!(notices_dir(&bin, v, false), None);
    fs::create_dir_all(dir.0.join("prefix/share/mlxtop")).unwrap();
    assert_eq!(
        notices_dir(&bin, v, false),
        Some(dir.0.join("prefix/share/mlxtop/3.1.0"))
    );
    let elsewhere = dir.write("tools/mlxtop", "");
    assert_eq!(notices_dir(&elsewhere, v, false), None);
    let exe = dir.write("Programs/mlxtop/mlxtop.exe", "");
    assert_eq!(notices_dir(&exe, v, true), None);
    dir.write("Programs/mlxtop/LICENSE", "MIT");
    assert_eq!(
        notices_dir(&exe, v, true),
        exe.parent().map(Path::to_path_buf)
    );
}

#[test]
fn executables_are_replaced_by_rename_and_windows_keeps_the_old_file_aside() {
    let dir = TempDir::new("update-replace");
    let replacement = dir.write("new/mlxtop", "new");
    for windows in [false, true] {
        let executable = dir.write(&format!("bin-{windows}/mlxtop"), "old");
        replace_executable(&executable, &replacement, windows).unwrap();
        assert_eq!(fs::read_to_string(&executable).unwrap(), "new");
        let left: Vec<_> = fs::read_dir(executable.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        let expected: &[&str] = if windows {
            &["mlxtop", "mlxtop.old"]
        } else {
            &["mlxtop"]
        };
        let mut left = left;
        left.sort();
        assert_eq!(left, expected);
    }
    let missing = dir.0.join("gone/mlxtop");
    assert!(replace_executable(&missing, &dir.0.join("absent"), false).is_err());
}

#[cfg(unix)]
mod end_to_end {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;

    const TARGET: &str = "aarch64-apple-darwin";

    /// A release whose `mlxtop` is a script printing `reports`.
    struct FakeRelease {
        responses: HashMap<String, Vec<u8>>,
    }

    impl FakeRelease {
        fn new(dir: &TempDir, tag: &str, reports: &str) -> Self {
            let version = tag.trim_start_matches('v');
            let name = format!("mlxtop-{version}-{TARGET}");
            let payload = dir.0.join("build").join(&name);
            fs::create_dir_all(payload.join("licenses")).unwrap();
            let binary = payload.join("mlxtop");
            fs::write(&binary, format!("#!/bin/sh\necho '{reports}'\n")).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(payload.join("LICENSE"), "MIT").unwrap();
            fs::write(payload.join("THIRD_PARTY_NOTICES.md"), "notices").unwrap();
            fs::write(payload.join("licenses/ring.txt"), "ISC").unwrap();
            let archive = format!("{name}.tar.gz");
            let status = Command::new("tar")
                .arg("-czf")
                .arg(dir.0.join(&archive))
                .arg("-C")
                .arg(dir.0.join("build"))
                .arg(&name)
                .status()
                .unwrap();
            assert!(status.success());
            let bytes = fs::read(dir.0.join(&archive)).unwrap();
            let sums = format!("{}  {archive}\n", sha256_hex(&bytes));
            let json = serde_json::json!([
                {"tag_name": "v2.0.0", "prerelease": false, "assets": []},
                {"tag_name": tag, "prerelease": version.contains("-rc."), "assets": [
                    {"name": archive, "browser_download_url": "https://github.com/dl/archive"},
                    {"name": "SHA256SUMS", "browser_download_url": "https://github.com/dl/sums"},
                ]},
            ]);
            Self {
                responses: HashMap::from([
                    (RELEASES_URL.to_string(), json.to_string().into_bytes()),
                    ("https://github.com/dl/archive".to_string(), bytes),
                    ("https://github.com/dl/sums".to_string(), sums.into_bytes()),
                ]),
            }
        }

        fn run(
            &mut self,
            executable: &Path,
            current: &str,
            rc: bool,
        ) -> (Result<(), String>, String) {
            let responses = &self.responses;
            let mut fetch = |url: &str| -> Result<Vec<u8>, Box<dyn Error>> {
                responses
                    .get(url)
                    .cloned()
                    .ok_or_else(|| format!("404 {url}").into())
            };
            let mut out = Vec::new();
            let result = Updater {
                fetch: &mut fetch,
                executable: executable.to_path_buf(),
                current,
                target: TARGET,
                windows: false,
            }
            .run(rc, &mut out)
            .map_err(|error| error.to_string());
            (result, String::from_utf8(out).unwrap())
        }
    }

    fn installed(dir: &TempDir) -> PathBuf {
        fs::create_dir_all(dir.0.join("prefix/share/mlxtop")).unwrap();
        dir.write("prefix/bin/mlxtop", "old binary")
    }

    #[test]
    fn update_installs_a_verified_newer_release_with_its_notices() {
        let dir = TempDir::new("update-e2e");
        let executable = installed(&dir);
        let mut release = FakeRelease::new(&dir, "v3.1.0", "mlxtop 3.1.0");
        let (result, out) = release.run(&executable, "3.0.0", false);
        assert_eq!(result, Ok(()));
        assert!(out.contains("Downloading mlxtop 3.1.0 (aarch64-apple-darwin)"));
        assert!(out.contains("Updated mlxtop 3.0.0 -> 3.1.0"));
        assert!(fs::read_to_string(&executable)
            .unwrap()
            .contains("mlxtop 3.1.0"));
        assert_eq!(
            fs::metadata(&executable).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let notices = dir.0.join("prefix/share/mlxtop/3.1.0");
        assert_eq!(fs::read_to_string(notices.join("LICENSE")).unwrap(), "MIT");
        assert_eq!(
            fs::read_to_string(notices.join("licenses/ring.txt")).unwrap(),
            "ISC"
        );

        // Running it again finds nothing newer; a newer local build stays put.
        let (result, out) = release.run(&executable, "3.1.0", false);
        assert_eq!(result, Ok(()));
        assert_eq!(out, "mlxtop 3.1.0 is up to date (release: 3.1.0).\n");
        let (_, out) = release.run(&executable, "3.2.0-rc.1", false);
        assert_eq!(
            out,
            "mlxtop 3.2.0-rc.1 is newer than the latest (release: 3.1.0).\n"
        );
    }

    #[test]
    fn release_candidates_install_only_with_rc() {
        let dir = TempDir::new("update-rc");
        let executable = installed(&dir);
        let mut release = FakeRelease::new(&dir, "v3.1.0-rc.1", "mlxtop 3.1.0-rc.1");
        let (_, out) = release.run(&executable, "2.0.0", false);
        assert_eq!(out, "mlxtop 2.0.0 is up to date (release: 2.0.0).\n");
        let (result, out) = release.run(&executable, "3.0.0", true);
        assert_eq!(result, Ok(()));
        assert!(out.contains("Updated mlxtop 3.0.0 -> 3.1.0-rc.1"));
        assert!(fs::read_to_string(&executable)
            .unwrap()
            .contains("3.1.0-rc.1"));
    }

    #[test]
    fn assets_outside_github_https_are_refused_with_nothing_downloaded() {
        let dir = TempDir::new("update-host");
        let executable = installed(&dir);
        for url in [
            "http://github.com/dl/archive",
            "https://example.com/dl/archive",
        ] {
            let mut release = FakeRelease::new(&dir, "v3.1.0", "mlxtop 3.1.0");
            let json = String::from_utf8(release.responses[RELEASES_URL].clone())
                .unwrap()
                .replace("https://github.com/dl/archive", url);
            release
                .responses
                .insert(RELEASES_URL.to_string(), json.into_bytes());
            let (result, _) = release.run(&executable, "3.0.0", false);
            assert!(result.unwrap_err().contains("outside github.com"), "{url}");
            assert_eq!(fs::read_to_string(&executable).unwrap(), "old binary");
        }
    }

    #[test]
    fn failed_checks_leave_the_installed_binary_untouched() {
        let dir = TempDir::new("update-fail");
        let executable = installed(&dir);
        let untouched = |dir: &TempDir| {
            assert_eq!(fs::read_to_string(&executable).unwrap(), "old binary");
            assert!(!dir.0.join("prefix/share/mlxtop/3.1.0").exists());
        };

        let mut tampered = FakeRelease::new(&dir, "v3.1.0", "mlxtop 3.1.0");
        tampered
            .responses
            .get_mut("https://github.com/dl/archive")
            .unwrap()
            .push(0);
        let (result, _) = tampered.run(&executable, "3.0.0", false);
        assert!(result.unwrap_err().contains("checksum mismatch"));
        untouched(&dir);

        let mut wrong = FakeRelease::new(&dir, "v3.1.0", "mlxtop 9.9.9");
        let (result, _) = wrong.run(&executable, "3.0.0", false);
        assert!(result.unwrap_err().contains("reports \"mlxtop 9.9.9\""));
        untouched(&dir);

        let mut unsummed = FakeRelease::new(&dir, "v3.1.0", "mlxtop 3.1.0");
        unsummed.responses.insert(
            "https://github.com/dl/sums".into(),
            b"nothing here".to_vec(),
        );
        let (result, _) = unsummed.run(&executable, "3.0.0", false);
        assert!(result.unwrap_err().contains("SHA256SUMS lists no checksum"));

        let mut corrupt = FakeRelease::new(&dir, "v3.1.0", "mlxtop 3.1.0");
        let garbage = b"not an archive".to_vec();
        let sums = format!("{}  mlxtop-3.1.0-{TARGET}.tar.gz\n", sha256_hex(&garbage));
        corrupt
            .responses
            .insert("https://github.com/dl/archive".into(), garbage);
        corrupt
            .responses
            .insert("https://github.com/dl/sums".into(), sums.into_bytes());
        let (result, _) = corrupt.run(&executable, "3.0.0", false);
        assert!(result.unwrap_err().contains("could not unpack"));

        let mut empty = FakeRelease::new(&dir, "v3.1.0", "mlxtop 3.1.0");
        empty.responses.insert(
            RELEASES_URL.into(),
            br#"[{"tag_name":"v3.1.0","assets":[]}]"#.to_vec(),
        );
        let (result, _) = empty.run(&executable, "3.0.0", false);
        assert_eq!(
            result.unwrap_err(),
            "release 3.1.0 has no mlxtop-3.1.0-aarch64-apple-darwin.tar.gz"
        );
        empty.responses.insert(RELEASES_URL.into(), b"[]".to_vec());
        let (result, _) = empty.run(&executable, "3.0.0", true);
        assert_eq!(
            result.unwrap_err(),
            "no published mlxtop release or release candidate found"
        );
        empty.responses.insert(RELEASES_URL.into(), b"{}".to_vec());
        let (result, _) = empty.run(&executable, "3.0.0", false);
        assert!(result
            .unwrap_err()
            .starts_with("unexpected GitHub releases response"));
        empty.responses.clear();
        let (result, _) = empty.run(&executable, "3.0.0", false);
        assert!(result.unwrap_err().starts_with("404"));
        untouched(&dir);

        let brewed = dir.write("Cellar/mlxtop/3.0.0/bin/mlxtop", "old binary");
        let (result, _) =
            FakeRelease::new(&dir, "v3.1.0", "mlxtop 3.1.0").run(&brewed, "3.0.0", false);
        assert!(result.unwrap_err().contains("brew upgrade mlxtop"));
    }
}
