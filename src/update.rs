// SPDX-License-Identifier: MIT
//! `mlxtop update`: replace this binary with the newest GitHub release.
use crate::cli::VERSION;
use serde::Deserialize;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RELEASES_URL: &str = "https://api.github.com/repos/maximpri/mlxtop/releases?per_page=30";
/// Release archives are a few MiB; anything far larger is not ours.
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;

/// An `X.Y.Z` or `X.Y.Z-rc.N` version. A final release sorts after its RCs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    rc: u64,
}

impl Version {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        fn number(part: &str) -> Option<u64> {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        }
        let (core, rc) = match text.split_once("-rc.") {
            Some((core, rc)) if !rc.starts_with('0') => {
                (core, number(rc).filter(|&rc| rc != u64::MAX)?)
            }
            Some(_) => return None,
            None => (text, u64::MAX),
        };
        let mut parts = core.split('.').map(number);
        let version = Self {
            major: parts.next()??,
            minor: parts.next()??,
            patch: parts.next()??,
            rc,
        };
        parts.next().is_none().then_some(version)
    }

    pub(crate) fn is_rc(self) -> bool {
        self.rc != u64::MAX
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.is_rc() {
            write!(f, "-rc.{}", self.rc)?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Asset {
    name: String,
    browser_download_url: String,
}

/// The newest published release; with `rc`, release candidates count too.
pub(crate) fn select_release(releases: Vec<Release>, rc: bool) -> Option<(Version, Release)> {
    releases
        .into_iter()
        .filter(|release| !release.draft && (rc || !release.prerelease))
        .filter_map(|release| {
            let version = Version::parse(release.tag_name.strip_prefix('v')?)?;
            (rc || !version.is_rc()).then_some((version, release))
        })
        .max_by_key(|(version, _)| *version)
}

/// The release archive name for the platform this binary was built for.
pub(crate) fn release_target() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("x86_64-unknown-linux-musl")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("aarch64-unknown-linux-musl")
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else {
        None
    }
}

fn archive_name(version: Version, target: &str) -> String {
    let extension = if target.contains("-windows-") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("mlxtop-{version}-{target}.{extension}")
}

/// The checksum `SHA256SUMS` publishes for `file`, in `sha256sum` format.
pub(crate) fn published_sha256(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let digest = fields.next()?;
        let name = fields.next()?;
        (name.strip_prefix('*').unwrap_or(name) == file
            && digest.len() == 64
            && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
    })
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Why this binary must be upgraded by the tool that installed it, if so.
pub(crate) fn managed_install(executable: &Path) -> Option<&'static str> {
    let in_dir = |name: &str| executable.components().any(|part| part.as_os_str() == name);
    if in_dir("Cellar") || in_dir(".linuxbrew") || in_dir("linuxbrew") {
        Some("mlxtop was installed with Homebrew; run: brew upgrade mlxtop")
    } else if executable
        .to_string_lossy()
        .replace('\\', "/")
        .contains("/mlxtop/rc/releases/")
    {
        Some("this is a private RC from scripts/rc.py; push a new RC instead")
    } else {
        None
    }
}

/// Where the installers keep license notices for `executable`: next to
/// `mlxtop.exe` on Windows, else `<prefix>/share/mlxtop/<version>` when an
/// installer already created `<prefix>/share/mlxtop`.
pub(crate) fn notices_dir(executable: &Path, version: Version, windows: bool) -> Option<PathBuf> {
    let bin = executable.parent()?;
    if windows {
        return bin.join("LICENSE").is_file().then(|| bin.to_path_buf());
    }
    if bin.file_name()? != "bin" {
        return None;
    }
    let share = bin.parent()?.join("share").join("mlxtop");
    share.is_dir().then(|| share.join(version.to_string()))
}

/// A unique scratch directory, removed when dropped.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new() -> io::Result<Self> {
        // The clock alone can repeat (macOS ticks in microseconds).
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "mlxtop-update-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{name}{suffix}"))
}

fn permission_hint(error: io::Error, directory: &Path) -> Box<dyn Error> {
    if error.kind() == io::ErrorKind::PermissionDenied {
        format!(
            "cannot write to {}; rerun with sudo or reinstall with the installer",
            directory.display()
        )
        .into()
    } else {
        error.into()
    }
}

/// Swap `replacement` in for `executable`. The new file is staged beside the
/// old one and renamed over it, so an interrupted update leaves the old binary.
/// Windows cannot replace a running program, but can rename it out of the way.
pub(crate) fn replace_executable(
    executable: &Path,
    replacement: &Path,
    windows: bool,
) -> Result<(), Box<dyn Error>> {
    let directory = executable.parent().ok_or("executable has no directory")?;
    let staged = sibling(executable, &format!(".update-{}", std::process::id()));
    fs::copy(replacement, &staged).map_err(|error| permission_hint(error, directory))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    }
    let result = if windows {
        let old = sibling(executable, ".old");
        let _ = fs::remove_file(&old);
        fs::rename(executable, &old).and_then(|()| {
            fs::rename(&staged, executable).inspect_err(|_| {
                let _ = fs::rename(&old, executable);
            })
        })
    } else {
        fs::rename(&staged, executable)
    };
    result.map_err(|error| {
        let _ = fs::remove_file(&staged);
        permission_hint(error, directory)
    })
}

fn copy_notices(payload: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for file in ["LICENSE", "THIRD_PARTY_NOTICES.md"] {
        fs::copy(payload.join(file), destination.join(file))?;
    }
    let licenses = destination.join("licenses");
    if licenses.exists() {
        fs::remove_dir_all(&licenses)?;
    }
    fs::create_dir(&licenses)?;
    for entry in fs::read_dir(payload.join("licenses"))? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), licenses.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Downloads a URL's body.
pub(crate) type Fetch<'a> = &'a mut dyn FnMut(&str) -> Result<Vec<u8>, Box<dyn Error>>;

/// Everything `update` needs from the outside world, so tests can stand in.
pub(crate) struct Updater<'a> {
    pub(crate) fetch: Fetch<'a>,
    pub(crate) executable: PathBuf,
    pub(crate) current: &'a str,
    pub(crate) target: &'a str,
    pub(crate) windows: bool,
}

impl Updater<'_> {
    pub(crate) fn run(&mut self, rc: bool, out: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        if let Some(reason) = managed_install(&self.executable) {
            return Err(reason.into());
        }
        if self.windows {
            let _ = fs::remove_file(sibling(&self.executable, ".old"));
        }
        let channel = if rc {
            "release or release candidate"
        } else {
            "release"
        };
        let releases: Vec<Release> = serde_json::from_slice(&(self.fetch)(RELEASES_URL)?)
            .map_err(|error| format!("unexpected GitHub releases response: {error}"))?;
        let (latest, release) = select_release(releases, rc)
            .ok_or_else(|| format!("no published mlxtop {channel} found"))?;
        let current = Version::parse(self.current);
        if current.is_some_and(|current| current >= latest) {
            let relation = if current == Some(latest) {
                "is up to date"
            } else {
                "is newer than the latest"
            };
            writeln!(
                out,
                "mlxtop {} {relation} ({channel}: {latest}).",
                self.current
            )?;
            return Ok(());
        }

        let archive = archive_name(latest, self.target);
        let asset = |name: &str| {
            release
                .assets
                .iter()
                .find(|asset| asset.name == name)
                .map(|asset| asset.browser_download_url.clone())
                .ok_or_else(|| format!("release {latest} has no {name}"))
                .and_then(|url| {
                    // Assets come from GitHub over HTTPS, nowhere else.
                    if url.starts_with("https://github.com/") {
                        Ok(url)
                    } else {
                        Err(format!("release {latest} lists {name} outside github.com"))
                    }
                })
        };
        let (archive_url, sums_url) = (asset(&archive)?, asset("SHA256SUMS")?);
        writeln!(out, "Downloading mlxtop {latest} ({})...", self.target)?;
        let bytes = (self.fetch)(&archive_url)?;
        let sums = String::from_utf8((self.fetch)(&sums_url)?)?;
        let expected = published_sha256(&sums, &archive)
            .ok_or_else(|| format!("SHA256SUMS lists no checksum for {archive}"))?;
        if sha256_hex(&bytes) != expected {
            return Err(format!("checksum mismatch for {archive}; nothing was changed").into());
        }

        let work = WorkDir::new()?;
        let archive_path = work.0.join(&archive);
        fs::write(&archive_path, &bytes)?;
        // bsdtar (macOS, Windows 10+) reads zip and gzip; GNU tar detects gzip.
        // An absolute path on macOS, where `sudo` keeps the caller's PATH.
        let status = Command::new(if cfg!(target_os = "macos") {
            "/usr/bin/tar"
        } else {
            "tar"
        })
        .arg("-xf")
        .arg(&archive_path)
        .arg("-C")
        .arg(&work.0)
        .status()
        .map_err(|error| format!("could not run tar to unpack {archive}: {error}"))?;
        if !status.success() {
            return Err(format!("tar could not unpack {archive}").into());
        }
        let payload = work.0.join(format!("mlxtop-{latest}-{}", self.target));
        let binary = payload.join(if self.windows { "mlxtop.exe" } else { "mlxtop" });
        let reported = Command::new(&binary)
            .arg("--version")
            .output()
            .map_err(|error| format!("could not run the downloaded mlxtop: {error}"))?;
        let reported = String::from_utf8_lossy(&reported.stdout);
        if reported.trim() != format!("mlxtop {latest}") {
            return Err(format!(
                "the downloaded binary reports {:?}, expected mlxtop {latest}",
                reported.trim()
            )
            .into());
        }

        replace_executable(&self.executable, &binary, self.windows)?;
        if let Some(notices) = notices_dir(&self.executable, latest, self.windows) {
            if let Err(error) = copy_notices(&payload, &notices) {
                writeln!(
                    out,
                    "Note: could not copy license notices to {}: {error}",
                    notices.display()
                )?;
            }
        }
        writeln!(
            out,
            "Updated mlxtop {} -> {latest} at {}",
            self.current,
            self.executable.display()
        )?;
        Ok(())
    }
}

fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let response = agent
        .get(url)
        .header(
            "Accept",
            "application/vnd.github+json, application/octet-stream",
        )
        .call();
    let mut response = match response {
        Err(ureq::Error::StatusCode(status @ (403 | 429))) => {
            return Err(format!(
                "GitHub refused the request (HTTP {status}); it allows 60 unauthenticated API requests per hour, try again later"
            )
            .into())
        }
        Err(error) => return Err(format!("could not download {url}: {error}").into()),
        Ok(response) => response,
    };
    Ok(response
        .body_mut()
        .with_config()
        .limit(MAX_DOWNLOAD_BYTES)
        .read_to_vec()
        .map_err(|error| format!("could not download {url}: {error}"))?)
}

/// `mlxtop update [--rc]` against GitHub, for the running binary.
pub(crate) fn update(rc: bool) -> Result<(), Box<dyn Error>> {
    let target = release_target().ok_or("no mlxtop release is built for this platform")?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(format!("mlxtop/{VERSION}"))
        .build()
        .into();
    let mut fetch = |url: &str| download(&agent, url);
    Updater {
        fetch: &mut fetch,
        executable,
        current: VERSION,
        target,
        windows: cfg!(windows),
    }
    .run(rc, &mut io::stdout().lock())
}

#[cfg(test)]
#[path = "tests/update.rs"]
mod tests;
