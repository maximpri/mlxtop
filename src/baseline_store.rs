// SPDX-License-Identifier: MIT
//! Usual speeds kept across sessions, so a new session judges its first
//! request against what this machine ran before, and an update that slowed
//! a model is named.
//!
//! Only numbers are kept: never a prompt, a request id, a command line or a
//! process name. For each kind of work (the machine by model identifier,
//! chip and RAM; the runtime; the model and its quantization; the runtime's
//! version; whether a draft runs; the band of context and how many requests
//! write at once) the latest speeds generation held outside slowdowns, as
//! the detector judges them: verify passes a second where the runtime
//! counts tokens per pass, else tokens a second, taken at the band's depth.
//! A speed adopted by the 30-second settle rule is never kept.
//!
//! One JSON file, readable by its owner only, in a folder only its owner can
//! open. It is written whole to a temporary file and renamed over the old
//! one, so an interrupted save leaves the last good file; a file this
//! version cannot use is set aside, never overwritten in place.
use crate::collector::Collector;
use crate::config::Config;
use crate::domain::{EventKind, Sample, SignalEvent, Tone};
use crate::host::{Host, Platform};
use crate::logging::{diagnostics_log, log_field};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, io};

/// The file's format. A newer mlxtop may write a higher one, which this one
/// sets aside rather than misread.
pub(crate) const SCHEMA: u32 = 1;
/// Speeds kept for each kind of work: as many as a usual value is taken over.
pub(crate) const KEPT: usize = crate::anomaly::WINDOW;
/// Kinds of work kept at most, the least recently seen going first: about
/// 150 KB at 300 speeds each.
pub(crate) const MAX_ENTRIES: usize = 64;
/// A kind of work not seen this long is forgotten.
pub(crate) const EXPIRY: Duration = Duration::from_secs(180 * 24 * 60 * 60);
/// New speeds are saved this often while generation runs, besides on a
/// model switch: quitting gives the sampler too little time to rely on.
pub(crate) const SAVE_EVERY: Duration = Duration::from_secs(300);
/// Speeds waiting for a save, at most: a disk that refuses every save must
/// not make them grow without end.
const PENDING_LIMIT: usize = 16 * KEPT;
/// A file larger than this is not one mlxtop wrote.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const FILE_NAME: &str = "baselines.json";
/// Where a file this version cannot use goes before it is replaced.
const SET_ASIDE: &str = "baselines.json.unread";

/// Where usual speeds are kept: mlxtop's folder in the place each platform
/// keeps an application's own lasting state. `~/Library/Application
/// Support/mlxtop` on macOS, Apple's place for files an app keeps for
/// itself (kept and backed up, unlike Caches); `$XDG_STATE_HOME/mlxtop` or
/// `~/.local/state/mlxtop` on Linux, for state that outlives a restart but
/// belongs to this machine; `%LOCALAPPDATA%\mlxtop` on Windows, this
/// machine's and not roaming. `None` without a home.
pub(crate) fn default_path() -> Option<PathBuf> {
    let platform = Platform::current();
    path_from(
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

pub(crate) fn path_from(
    state_home: Option<String>,
    home: Option<PathBuf>,
    platform: Platform,
) -> Option<PathBuf> {
    let state = state_home
        .map(|dir| PathBuf::from(dir.trim()))
        .filter(|dir| !dir.as_os_str().is_empty());
    let folder = match platform {
        Platform::MacOs => home?.join("Library/Application Support"),
        // A relative state folder is invalid and ignored, as XDG says.
        Platform::Linux => state
            .filter(|dir| dir.is_absolute())
            .or_else(|| home.map(|home| home.join(".local/state")))?,
        Platform::Windows => state.or_else(|| home.map(|home| home.join("AppData/Local")))?,
    };
    Some(folder.join("mlxtop").join(FILE_NAME))
}

/// `path` as a person reads it, the home folder shown as `~`.
pub(crate) fn shown(path: &Path) -> String {
    crate::config::home_dir()
        .and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf))
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

/// The machine's model identifier: `Mac16,9` from `hw.model` on a Mac, the
/// DMI product name on a PC, a board's device-tree model elsewhere on Linux.
pub(crate) fn machine_model(host: &dyn Host, platform: Platform) -> Option<String> {
    let text = match platform {
        Platform::MacOs => host.command("/usr/sbin/sysctl", &["-n", "hw.model"]),
        Platform::Linux => host
            .read_file(Path::new("/sys/class/dmi/id/product_name"))
            .or_else(|| host.read_file(Path::new("/proc/device-tree/model"))),
        Platform::Windows => None,
    }?;
    let model: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(80)
        .collect::<String>()
        .trim()
        .to_owned();
    // Placeholder DMI strings say nothing about the machine.
    let placeholder = [
        "",
        "To Be Filled By O.E.M.",
        "System Product Name",
        "Default string",
    ];
    (!placeholder.contains(&model.as_str())).then_some(model)
}

/// The machine speeds were measured on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Machine {
    /// Its model identifier (`Mac16,9`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// The chip (`Apple M4 Max`), or the cards (`2× NVIDIA GeForce RTX 4090`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) chip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gpu_cores: Option<u16>,
    /// RAM in whole GiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) ram_gb: Option<u64>,
}

impl Machine {
    pub(crate) fn of(sample: &Sample) -> Self {
        let chip = if !sample.gpus.is_empty() && !sample.apple_gpu() {
            let mut names: Vec<&str> = sample.gpus.iter().map(|gpu| gpu.name.as_str()).collect();
            names.sort_unstable();
            Some(
                names
                    .chunk_by(|a, b| a == b)
                    .map(|same| match same.len() {
                        1 => same[0].to_owned(),
                        count => format!("{count}× {}", same[0]),
                    })
                    .collect::<Vec<_>>()
                    .join(" + "),
            )
        } else {
            sample
                .metal
                .device_name
                .clone()
                .or_else(|| sample.metal.architecture.clone())
        };
        Self {
            model: sample.metal.machine_model.clone(),
            chip,
            gpu_cores: sample.metal.gpu_cores,
            ram_gb: (sample.total_memory > 0).then(|| (sample.total_memory + (1 << 29)) >> 30),
        }
    }

    /// The same machine: chip, GPU cores and RAM alike, and the model
    /// identifier too where both know it (a recording made before it was
    /// read knows none).
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.chip == other.chip
            && self.gpu_cores == other.gpu_cores
            && self.ram_gb == other.ram_gb
            && (self.model.is_none() || other.model.is_none() || self.model == other.model)
    }
}

/// What usual speeds are kept for: a model served by a runtime on a machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) machine: Machine,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) quantization: Option<String>,
    /// The runtime with its version (`oMLX 0.7.3 · Splash 1.5.0`).
    pub(crate) runtime: Option<String>,
    pub(crate) draft: Option<bool>,
}

/// How a newer reading of the identity differs from the one kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Change {
    Same,
    /// Something unknown became known: the same work, better named.
    Completed,
    /// Something known became something else: the runtime was updated, or
    /// the model requantized, under the same name.
    Changed,
}

/// Takes `newer` into `kept` where it says something; how that changed it.
fn merge<T: PartialEq + Clone>(kept: &mut Option<T>, newer: &Option<T>) -> Change {
    match (kept.as_ref(), newer) {
        (_, None) => Change::Same,
        (Some(old), Some(new)) if old == new => Change::Same,
        (old, Some(new)) => {
            let change = if old.is_some() {
                Change::Changed
            } else {
                Change::Completed
            };
            *kept = Some(new.clone());
            change
        }
    }
}

impl Identity {
    /// What `sample` is serving on this machine; `None` with no local model.
    pub(crate) fn of(sample: &Sample) -> Option<Self> {
        let unnamed = |name: &str| matches!(name.trim(), "" | "none" | "not detected");
        if sample.llm_remote || unnamed(&sample.llm_provider) || unnamed(&sample.llm_model) {
            return None;
        }
        let facts = &sample.model;
        // A runtime names its draft by drafting: acceptance or more than
        // one token a pass is a draft whatever the facts say.
        let drafting = facts.draft == Some(true)
            || sample.llm_draft_acceptance.is_some()
            || sample
                .llm_tokens_per_pass
                .is_some_and(|tokens| tokens > 1.0);
        Some(Self {
            machine: Machine::of(sample),
            provider: sample.llm_provider.clone(),
            model: model_name(&sample.llm_model),
            quantization: facts.quantization.clone(),
            runtime: facts.runtime_version.clone(),
            draft: if drafting { Some(true) } else { facts.draft },
        })
    }

    /// This identity with what `newer`, for the same model, adds. What a
    /// reading leaves unknown keeps its value, and a draft once seen stays
    /// seen: a runtime shows its draft only while it drafts.
    pub(crate) fn merged(&self, newer: &Self) -> (Self, Change) {
        let mut next = self.clone();
        let mut change = merge(&mut next.quantization, &newer.quantization)
            .max(merge(&mut next.runtime, &newer.runtime))
            .max(merge(&mut next.machine.model, &newer.machine.model))
            .max(merge(&mut next.machine.chip, &newer.machine.chip))
            .max(merge(&mut next.machine.gpu_cores, &newer.machine.gpu_cores))
            .max(merge(&mut next.machine.ram_gb, &newer.machine.ram_gb));
        if next.draft != Some(true) {
            change = change.max(merge(&mut next.draft, &newer.draft));
        }
        (next, change)
    }

    /// Whether `entry` holds speeds of this model on this machine, whatever
    /// the runtime's version.
    fn holds(&self, entry: &Entry) -> bool {
        entry.machine.same(&self.machine)
            && entry.provider == self.provider
            && entry.model == self.model
            && entry.quantization == self.quantization
            && entry.draft == self.draft
            && self.compares(entry.per)
    }

    /// Whether speeds counted `per` say what this machine does from one
    /// session to the next. With a draft, tokens a second are also how well
    /// it guesses this session's text, and another session is another
    /// text: only verify passes compare.
    pub(crate) fn compares(&self, per: Per) -> bool {
        per == Per::Pass || self.draft != Some(true)
    }
}

/// A model's name as kept: a path it was loaded from keeps only its file
/// name, which says the model without the folders around it.
fn model_name(id: &str) -> String {
    let id = id.trim();
    let path = id.starts_with(['/', '~', '\\'])
        || id.as_bytes().get(1..3) == Some(b":\\".as_slice())
        || id.as_bytes().get(1..3) == Some(b":/".as_slice());
    let name = if path {
        id.rsplit(['/', '\\']).next().unwrap_or(id)
    } else {
        id
    };
    name.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// Bands of context speeds are kept by: depth slows decode, and within a
/// band the bytes model takes each speed to the band's depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Band {
    #[serde(rename = "<1k")]
    Under1k,
    #[serde(rename = "1k–4k")]
    To4k,
    #[serde(rename = "4k–16k")]
    To16k,
    #[serde(rename = "16k–64k")]
    To64k,
    #[serde(rename = "64k+")]
    From64k,
    /// The runtime does not say how long its requests are.
    #[serde(rename = "unknown")]
    Unknown,
}

impl Band {
    pub(crate) fn of(context: Option<u64>) -> Self {
        match context {
            None => Self::Unknown,
            Some(tokens) if tokens < 1_024 => Self::Under1k,
            Some(tokens) if tokens < 4_096 => Self::To4k,
            Some(tokens) if tokens < 16_384 => Self::To16k,
            Some(tokens) if tokens < 65_536 => Self::To64k,
            Some(_) => Self::From64k,
        }
    }

    /// The context a band's speeds are taken at: its middle, within a
    /// factor of two of the whole band.
    pub(crate) fn depth(self) -> Option<u64> {
        match self {
            Self::Under1k => Some(512),
            Self::To4k => Some(2_048),
            Self::To16k => Some(8_192),
            Self::To64k => Some(32_768),
            Self::From64k => Some(131_072),
            Self::Unknown => None,
        }
    }
}

/// What a kept speed counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Per {
    /// Verify passes a second: the runtime counts tokens per pass.
    Pass,
    Token,
}

/// A kind of work speeds are kept for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Kind {
    pub(crate) band: Band,
    /// Requests writing at once, 1 to 4 (4 or more).
    pub(crate) requests: u64,
    pub(crate) per: Per,
}

/// One kind of work's speeds, as the file holds them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) machine: Machine,
    pub(crate) provider: String,
    pub(crate) model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) quantization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) draft: Option<bool>,
    pub(crate) context: Band,
    /// The context the speeds are taken at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) depth: Option<u64>,
    pub(crate) requests: u64,
    pub(crate) per: Per,
    /// When speeds were last added, in seconds since the Unix epoch.
    pub(crate) seen: u64,
    /// The latest speeds, oldest first.
    pub(crate) speeds: Vec<f64>,
}

impl Entry {
    fn kind(&self) -> Kind {
        Kind {
            band: self.context,
            requests: self.requests,
            per: self.per,
        }
    }

    fn fresh(&self, now: SystemTime) -> bool {
        let seen = UNIX_EPOCH + Duration::from_secs(self.seen);
        now.duration_since(seen).map_or(true, |age| age < EXPIRY)
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    schema: u32,
    #[serde(default)]
    baselines: Vec<Entry>,
}

/// What reading the file found.
#[derive(Debug, PartialEq)]
pub(crate) enum Read {
    Missing,
    Entries(Vec<Entry>),
    /// Not a file this version can use, and why.
    Unusable(String),
}

/// The file at `path`, its entries cleaned: speeds finite and positive, the
/// latest `KEPT` of each, the most recently seen `MAX_ENTRIES` entries.
pub(crate) fn read(path: &Path) -> Read {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Read::Missing,
        Err(error) => return Read::Unusable(error.to_string()),
    };
    if metadata.len() > MAX_FILE_BYTES {
        return Read::Unusable(format!("larger than {} MiB", MAX_FILE_BYTES >> 20));
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => return Read::Unusable(error.to_string()),
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(error) => return Read::Unusable(format!("not JSON: {error}")),
    };
    match value.get("schema").and_then(serde_json::Value::as_u64) {
        None => return Read::Unusable("no schema number".into()),
        Some(schema) if schema > u64::from(SCHEMA) => {
            return Read::Unusable(format!("schema {schema} is newer than {SCHEMA}"))
        }
        Some(_) => {}
    }
    match serde_json::from_value::<File>(value) {
        Ok(file) => Read::Entries(clean(file.baselines)),
        Err(error) => Read::Unusable(format!("not a baseline file: {error}")),
    }
}

fn clean(mut entries: Vec<Entry>) -> Vec<Entry> {
    for entry in &mut entries {
        entry
            .speeds
            .retain(|speed| speed.is_finite() && *speed > 0.0);
        let extra = entry.speeds.len().saturating_sub(KEPT);
        entry.speeds.drain(..extra);
    }
    entries.retain(|entry| !entry.speeds.is_empty() && (1..=4).contains(&entry.requests));
    if entries.len() > MAX_ENTRIES {
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.seen));
        entries.truncate(MAX_ENTRIES);
    }
    entries
}

/// Write `entries` to `path` whole: to a private temporary file beside it,
/// flushed to disk, then renamed over it. The folder is created readable by
/// its owner only. Returns whether the file is new.
pub(crate) fn write(path: &Path, entries: &[Entry]) -> io::Result<bool> {
    let folder = path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    private_folder(folder)?;
    let created = !path.exists();
    let mut text = serde_json::to_string(&File {
        schema: SCHEMA,
        baselines: entries.to_vec(),
    })?;
    text.push('\n');
    let temporary = folder.join(format!(".{FILE_NAME}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    // The rename itself reaches the disk with the folder.
    if let Ok(folder) = fs::File::open(folder) {
        let _ = folder.sync_all();
    }
    Ok(created)
}

/// `folder` and the folders above it that are missing, each readable by its
/// owner only; `folder` is made so if it already exists.
fn private_folder(folder: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(folder)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(folder)?.permissions().mode() & 0o777;
        if mode != 0o700 {
            fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Speeds of one kind of work kept from earlier sessions, for the engine to
/// judge the work this session has not yet done against.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Seed {
    pub(crate) kind: Kind,
    /// Oldest first, in what `kind.per` counts, at `kind.band`'s depth.
    pub(crate) speeds: Vec<f64>,
    /// The runtime and version they were measured on.
    pub(crate) runtime: Option<String>,
}

/// What a new reading of the identity asks of the engine.
#[derive(Debug, PartialEq)]
pub(crate) enum Follow {
    Same,
    /// Judge against these from now on, keeping what this session learned:
    /// a model first seen, or one now better known.
    Seed(Vec<Seed>),
    /// Start over with these: the runtime or the model changed under the
    /// same name, and what this session learned is of the old one.
    Restart(Vec<Seed>),
}

/// The engine's link to the file: what it judges, the speeds it learned and
/// has not yet saved, and when it last saved.
#[derive(Debug)]
pub(crate) struct Baselines {
    path: PathBuf,
    /// Settings turns it off: the file is then neither read nor written.
    pub(crate) on: bool,
    /// A replay reads the file and never writes it.
    writes: bool,
    identity: Option<Identity>,
    /// Speeds learned outside slowdowns since the last save.
    pending: Vec<(Kind, f64)>,
    /// When the first of them came: they are saved `SAVE_EVERY` after.
    since: Option<SystemTime>,
    /// Why the file is not used, once said in the log.
    unusable: Option<String>,
    /// A save failed and was logged; the next failure is not.
    failing: bool,
    /// Whether this session has looked for the file before saving: one
    /// that does not exist yet is made at once, so its notice is seen
    /// while the session runs rather than written at exit.
    looked: bool,
    /// Said once in the Journal, the first time the file is made.
    pub(crate) notice: Option<String>,
}

impl Baselines {
    /// Kept in the file at `path`, read and written while `on`.
    pub(crate) fn new(path: PathBuf, on: bool) -> Self {
        Self {
            path,
            on,
            writes: true,
            identity: None,
            pending: Vec::new(),
            since: None,
            unusable: None,
            failing: false,
            looked: false,
            notice: None,
        }
    }

    /// The file at `path` for a replay: read, never written. An error when
    /// it cannot be used, saying why.
    pub(crate) fn read_only(path: &Path) -> io::Result<Self> {
        match read(path) {
            Read::Entries(_) => Ok(Self {
                writes: false,
                ..Self::new(path.to_path_buf(), true)
            }),
            Read::Missing => Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{}: no such file", path.display()),
            )),
            Read::Unusable(reason) => Err(io::Error::other(format!(
                "{} is not usable: {reason}",
                path.display()
            ))),
        }
    }

    /// The runtime and version of the model being judged.
    pub(crate) fn runtime(&self) -> Option<&str> {
        self.identity.as_ref()?.runtime.as_deref()
    }

    /// Follow what `sample` serves: speeds to judge against when its model
    /// is first seen or better known, and a restart, with what this session
    /// learned saved first, when the runtime or model changed under it.
    pub(crate) fn follow(&mut self, sample: &Sample) -> Follow {
        if !self.on {
            return Follow::Same;
        }
        let Some(newer) = Identity::of(sample) else {
            return Follow::Same;
        };
        let Some(kept) = self
            .identity
            .as_ref()
            .filter(|kept| kept.provider == newer.provider && kept.model == newer.model)
        else {
            // A model first seen, or another one: the last one's speeds
            // are saved first.
            self.flush();
            self.identity = Some(newer);
            return Follow::Seed(self.seeds());
        };
        let (next, change) = kept.merged(&newer);
        match change {
            Change::Same => Follow::Same,
            Change::Completed => {
                self.identity = Some(next);
                Follow::Seed(self.seeds())
            }
            Change::Changed => {
                self.flush();
                self.identity = Some(next);
                Follow::Restart(self.seeds())
            }
        }
    }

    /// The engine starts over: what it learned is saved, and the model is
    /// seen anew on its next sample.
    pub(crate) fn restart(&mut self) {
        self.flush();
        self.identity = None;
    }

    /// The file's speeds for the model being judged, a window per kind of
    /// work: those measured on this runtime version, else on the version
    /// seen most recently, which an update replaced.
    fn seeds(&mut self) -> Vec<Seed> {
        let Some(identity) = self.identity.clone() else {
            return Vec::new();
        };
        let entries = self.entries();
        let now = SystemTime::now();
        let mut held: Vec<&Entry> = entries
            .iter()
            .filter(|entry| identity.holds(entry))
            .filter(|entry| !self.writes || entry.fresh(now))
            .collect();
        // This version first, then the others by when they were seen.
        held.sort_by_key(|entry| {
            (
                entry.runtime != identity.runtime,
                std::cmp::Reverse(entry.seen),
            )
        });
        let mut seeds: Vec<Seed> = Vec::new();
        for entry in held {
            let kind = entry.kind();
            if seeds.iter().any(|seed| seed.kind == kind) {
                continue;
            }
            seeds.push(Seed {
                kind,
                speeds: entry.speeds.clone(),
                runtime: entry.runtime.clone(),
            });
        }
        seeds
    }

    /// What the file holds now: another mlxtop may have saved since. A file
    /// this version cannot use is said once in the log and holds nothing.
    fn entries(&mut self) -> Vec<Entry> {
        match read(&self.path) {
            Read::Entries(entries) => {
                self.unusable = None;
                entries
            }
            Read::Missing => {
                self.unusable = None;
                Vec::new()
            }
            Read::Unusable(reason) => {
                if self.unusable.as_deref() != Some(reason.as_str()) {
                    diagnostics_log(
                        "WARN",
                        "baselines_ignored",
                        format!(
                            "path={} reason={} set_aside={SET_ASIDE}",
                            log_field(&self.path.display().to_string()),
                            log_field(&reason)
                        ),
                    );
                    self.unusable = Some(reason);
                }
                Vec::new()
            }
        }
    }

    /// One speed learned outside a slowdown, in what `kind.per` counts and
    /// at `kind.band`'s depth.
    pub(crate) fn learned(&mut self, kind: Kind, speed: f64, at: Option<SystemTime>) {
        let compares = self
            .identity
            .as_ref()
            .is_none_or(|identity| identity.compares(kind.per));
        if !self.on || !self.writes || !compares || !speed.is_finite() || speed <= 0.0 {
            return;
        }
        self.pending.push((kind, speed));
        let extra = self.pending.len().saturating_sub(PENDING_LIMIT);
        self.pending.drain(..extra);
        if self.since.is_none() {
            self.since = Some(at.unwrap_or_else(SystemTime::now));
        }
    }

    /// Save what was learned once it has waited `SAVE_EVERY` (`at` is the
    /// sample's time), or at once while no file exists yet.
    pub(crate) fn save_if_due(&mut self, at: Option<SystemTime>) {
        let Some(since) = self.since else {
            return;
        };
        let first = !self.looked && !self.path.exists();
        self.looked = true;
        let due = first
            || at
                .unwrap_or_else(SystemTime::now)
                .duration_since(since)
                .is_ok_and(|waited| waited >= SAVE_EVERY);
        if due {
            self.flush();
            // A save that failed is tried again a period later, not on
            // every sample.
            if !self.pending.is_empty() {
                self.since = at.or_else(|| Some(SystemTime::now()));
            }
        }
    }

    /// Save what was learned into the file, as it is on disk now, so speeds
    /// another mlxtop saved meanwhile stay. Each kind keeps its latest
    /// `KEPT`; kinds not seen for `EXPIRY` go.
    pub(crate) fn flush(&mut self) {
        if !self.on || !self.writes || self.pending.is_empty() {
            return;
        }
        let Some(identity) = self.identity.clone() else {
            self.pending.clear();
            self.since = None;
            return;
        };
        let mut entries = self.entries();
        let now = SystemTime::now();
        let seen = now
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        for (kind, speed) in &self.pending {
            let index = entries.iter().position(|entry| {
                identity.holds(entry) && entry.runtime == identity.runtime && entry.kind() == *kind
            });
            let entry = match index {
                Some(index) => &mut entries[index],
                None => {
                    entries.push(Entry {
                        machine: identity.machine.clone(),
                        provider: identity.provider.clone(),
                        model: identity.model.clone(),
                        quantization: identity.quantization.clone(),
                        runtime: identity.runtime.clone(),
                        draft: identity.draft,
                        context: kind.band,
                        depth: kind.band.depth(),
                        requests: kind.requests,
                        per: kind.per,
                        seen,
                        speeds: Vec::new(),
                    });
                    entries.last_mut().expect("just pushed")
                }
            };
            // Hundredths: a usual speed is a median, and 0.01 is far
            // inside its spread.
            entry.speeds.push((speed * 100.0).round() / 100.0);
            let extra = entry.speeds.len().saturating_sub(KEPT);
            entry.speeds.drain(..extra);
            entry.seen = seen;
        }
        entries.retain(|entry| entry.fresh(now));
        let entries = clean(entries);
        match self.replace(&entries) {
            Ok(created) => {
                self.pending.clear();
                self.since = None;
                if self.failing {
                    diagnostics_log("INFO", "baselines_saved", "after_failure=true");
                }
                self.failing = false;
                if created {
                    let notice = format!(
                        "usual speeds now kept in {} · Settings (4) turns this off",
                        shown(&self.path)
                    );
                    diagnostics_log(
                        "INFO",
                        "baselines_created",
                        format!("path={}", log_field(&self.path.display().to_string())),
                    );
                    self.notice = Some(notice);
                }
            }
            Err(error) => {
                if !self.failing {
                    diagnostics_log(
                        "WARN",
                        "baselines_save_failed",
                        format!(
                            "path={} error={}",
                            log_field(&self.path.display().to_string()),
                            log_field(&error.to_string())
                        ),
                    );
                }
                self.failing = true;
            }
        }
    }

    /// Write `entries`, first setting aside a file this version could not
    /// use, so that nothing it held is lost to this save.
    fn replace(&mut self, entries: &[Entry]) -> io::Result<bool> {
        if self.unusable.take().is_some() && self.path.exists() {
            fs::rename(&self.path, self.path.with_file_name(SET_ASIDE))?;
        }
        write(&self.path, entries)
    }

    /// Forget every usual speed kept for `machine`, and what was learned and
    /// not yet saved.
    pub(crate) fn forget(&mut self, machine: &Machine) {
        self.pending.clear();
        self.since = None;
        if !self.on || !self.writes {
            return;
        }
        if let Read::Entries(mut entries) = read(&self.path) {
            entries.retain(|entry| !entry.machine.same(machine));
            match write(&self.path, &entries) {
                Ok(_) => diagnostics_log("INFO", "baselines_forgotten", "machine=this"),
                Err(error) => diagnostics_log(
                    "WARN",
                    "baselines_forget_failed",
                    format!("error={}", log_field(&error.to_string())),
                ),
            }
        }
    }
}

/// The finding against earlier sessions: how far below their usual this
/// work runs, and the runtime's version their usual was measured on and
/// the one now, each where known.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BelowEarlier {
    pub(crate) percent: f64,
    pub(crate) then: Option<String>,
    pub(crate) now: Option<String>,
}

impl BelowEarlier {
    /// The finding in one sentence, with the update between where there
    /// was one.
    pub(crate) fn sentence(&self) -> String {
        let mut text = format!(
            "This model now generates {:.0}% below its usual from earlier sessions at this depth",
            self.percent
        );
        if let Some((then, now)) = self.update() {
            text.push_str(&format!(", since {}", version_change(then, now)));
        }
        text
    }

    /// The update between, where the runtime's version is known then and
    /// now and differs.
    pub(crate) fn update(&self) -> Option<(&str, &str)> {
        self.then
            .as_deref()
            .zip(self.now.as_deref())
            .filter(|(then, now)| then != now)
    }
}

/// How the runtime went from `old` to `new`: `oMLX 0.7.3 became 0.8.0`
/// where one part changed its version, else the two names whole.
pub(crate) fn version_change(old: &str, new: &str) -> String {
    let before: Vec<&str> = old.split(" · ").collect();
    let after: Vec<&str> = new.split(" · ").collect();
    let gone: Vec<&str> = before
        .iter()
        .copied()
        .filter(|part| !after.contains(part))
        .collect();
    let came: Vec<&str> = after
        .iter()
        .copied()
        .filter(|part| !before.contains(part))
        .collect();
    if let ([gone], [came]) = (gone.as_slice(), came.as_slice()) {
        if let Some(((name, from), (same, to))) = gone.split_once(' ').zip(came.split_once(' ')) {
            if name == same {
                return format!("{name} {from} became {to}");
            }
        }
    }
    format!("{old} became {new}")
}

impl Collector {
    /// Keep usual speeds across sessions in the file at `path`, read and
    /// written while `on`, and read the machine's model identifier they are
    /// kept for (recordings carry it too).
    pub(crate) fn keep_baselines(&mut self, path: Option<PathBuf>, on: bool) {
        self.metal.machine_model = machine_model(self.host.as_ref(), self.platform);
        self.correlation.baselines = path.map(|path| Box::new(Baselines::new(path, on)));
    }

    /// Settings turned usual speeds across sessions on or off. Off drops
    /// what was learned and not saved; on judges the model being watched
    /// against the file from its next sample.
    pub(crate) fn apply_baselines(&mut self, config: &Config) {
        let on = config.baselines != Some(false);
        let Some(baselines) = self.correlation.baselines.as_deref_mut() else {
            return;
        };
        if baselines.on == on {
            return;
        }
        if !on {
            baselines.pending.clear();
            baselines.since = None;
        }
        baselines.on = on;
        baselines.identity = None;
        self.correlation.speed.forget_earlier();
    }

    /// Forget the usual speeds kept for this machine; the session itself is
    /// reset by the caller.
    pub(crate) fn forget_baselines(&mut self) {
        let machine = Machine::of(&self.current);
        if let Some(baselines) = self.correlation.baselines.as_deref_mut() {
            baselines.forget(&machine);
        }
    }

    /// Save what was learned now, as when sampling stops.
    pub(crate) fn save_baselines(&mut self) {
        if let Some(baselines) = self.correlation.baselines.as_deref_mut() {
            baselines.flush();
        }
    }

    /// The one-line notice, the first time the file is made, once.
    pub(crate) fn baselines_notice(&mut self) -> Option<String> {
        self.correlation
            .baselines
            .as_deref_mut()
            .and_then(|baselines| baselines.notice.take())
    }

    /// The notice in the Journal, where the session's other system rows
    /// are.
    pub(crate) fn journal_baselines(&mut self) {
        let Some(notice) = self.baselines_notice() else {
            return;
        };
        self.view.signals.push_back(SignalEvent {
            time: self.view.current.updated.clone(),
            recorded_at: SystemTime::now(),
            kind: EventKind::System,
            state: "SYSTEM".into(),
            summary: notice,
            tone: Tone::Cyan,
        });
        let limit = self.history_limit.clamp(40, 240);
        while self.view.signals.len() > limit {
            self.view.signals.pop_front();
        }
    }
}

#[cfg(test)]
#[path = "tests/baseline_store.rs"]
mod tests;
