// SPDX-License-Identifier: MIT
//! Usual speeds kept across sessions: where the file lives, what it keys on,
//! how it is written and read, and what it never holds.
use super::*;
use crate::model_facts::ModelFacts;
use crate::test_support::{FakeHost, TempDir};

/// A Mac Studio serving `model` through oMLX `version`, with no draft.
fn serving(model: &str, version: &str) -> Sample {
    Sample {
        llm_provider: "oMLX".into(),
        llm_model: model.into(),
        total_memory: 36 * (1 << 30),
        metal: crate::domain::MetalTelemetry {
            device_name: Some("Apple M4 Max".into()),
            gpu_cores: Some(32),
            machine_model: Some("Mac16,9".into()),
            ..Default::default()
        },
        model: ModelFacts {
            quantization: Some("4-bit".into()),
            runtime_version: Some(version.into()),
            draft: Some(false),
            ..ModelFacts::default()
        },
        ..Sample::default()
    }
}

fn identity(version: &str) -> Identity {
    Identity::of(&serving("Qwen3.8-27B-oQ4e-mtp", version)).unwrap()
}

/// What the file holds for `identity`: `speeds` of one lone request
/// 16k–64k deep, per token, seen `seen` seconds after the epoch.
fn entry(identity: &Identity, speeds: Vec<f64>, seen: u64) -> Entry {
    Entry {
        machine: identity.machine.clone(),
        provider: identity.provider.clone(),
        model: identity.model.clone(),
        quantization: identity.quantization.clone(),
        runtime: identity.runtime.clone(),
        draft: identity.draft,
        context: Band::To64k,
        depth: Band::To64k.depth(),
        requests: 1,
        per: Per::Token,
        seen,
        speeds,
    }
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

const KIND: Kind = Kind {
    band: Band::To64k,
    requests: 1,
    per: Per::Token,
};

#[test]
fn the_file_lives_where_each_platform_keeps_an_apps_own_state() {
    let home = Some(PathBuf::from("/Users/u"));
    assert_eq!(
        path_from(Some("/state".into()), home.clone(), Platform::MacOs),
        Some(PathBuf::from(
            "/Users/u/Library/Application Support/mlxtop/baselines.json"
        )),
        "a Mac keeps it in Application Support whatever XDG says"
    );
    assert_eq!(
        path_from(Some("/state".into()), home.clone(), Platform::Linux),
        Some(PathBuf::from("/state/mlxtop/baselines.json"))
    );
    // Unset, blank or relative, XDG_STATE_HOME falls back to its default.
    for state in [None, Some("  ".into()), Some("relative/state".into())] {
        assert_eq!(
            path_from(state, home.clone(), Platform::Linux),
            Some(PathBuf::from("/Users/u/.local/state/mlxtop/baselines.json"))
        );
    }
    assert_eq!(
        path_from(
            Some(r"C:\Users\u\AppData\Local".into()),
            None,
            Platform::Windows
        )
        .map(|path| path.ends_with("mlxtop/baselines.json")),
        Some(true)
    );
    assert_eq!(
        path_from(None, home, Platform::Windows),
        Some(PathBuf::from(
            "/Users/u/AppData/Local/mlxtop/baselines.json"
        ))
    );
    // With no home there is nowhere to keep them.
    assert_eq!(path_from(None, None, Platform::MacOs), None);
    assert_eq!(path_from(None, None, Platform::Linux), None);
    assert!(default_path().is_some_and(|path| path.ends_with("mlxtop/baselines.json")));
    // Shown with the home folder as ~.
    let home = crate::config::home_dir().unwrap();
    assert_eq!(
        shown(&home.join("Library/Application Support/mlxtop/baselines.json")),
        "~/Library/Application Support/mlxtop/baselines.json"
    );
    assert_eq!(shown(Path::new("/elsewhere/b.json")), "/elsewhere/b.json");
}

#[test]
fn the_machine_is_known_by_its_model_identifier_chip_and_ram() {
    let mac = FakeHost::default().command("/usr/sbin/sysctl -n hw.model", "Mac16,9\n");
    assert_eq!(
        machine_model(&mac, Platform::MacOs).as_deref(),
        Some("Mac16,9")
    );
    // A PC's DMI name, unless it is a placeholder; a board's device tree.
    let pc = FakeHost::default().file("/sys/class/dmi/id/product_name", "Framework Desktop\n");
    assert_eq!(
        machine_model(&pc, Platform::Linux).as_deref(),
        Some("Framework Desktop")
    );
    let blank =
        FakeHost::default().file("/sys/class/dmi/id/product_name", "To Be Filled By O.E.M.\n");
    assert_eq!(machine_model(&blank, Platform::Linux), None);
    let board = FakeHost::default().file("/proc/device-tree/model", "NVIDIA Jetson AGX Orin\0");
    assert_eq!(
        machine_model(&board, Platform::Linux).as_deref(),
        Some("NVIDIA Jetson AGX Orin")
    );
    assert_eq!(machine_model(&FakeHost::default(), Platform::MacOs), None);
    assert_eq!(machine_model(&mac, Platform::Windows), None);

    let machine = Machine::of(&serving("m", "oMLX 0.7.3"));
    assert_eq!(
        machine,
        Machine {
            model: Some("Mac16,9".into()),
            chip: Some("Apple M4 Max".into()),
            gpu_cores: Some(32),
            ram_gb: Some(36),
        }
    );
    // Cards are named and counted; two alike are one name.
    let mut rig = Sample {
        total_memory: 128 * (1 << 30),
        ..Sample::default()
    };
    rig.gpus = crate::gpu::parse(
        "0, GPU-a, NVIDIA GeForce RTX 4090, 50, 24564, 1000, 40\n1, GPU-b, NVIDIA GeForce RTX 4090, 50, 24564, 1000, 40\n2, GPU-c, NVIDIA GeForce RTX 3090, 50, 24564, 1000, 40\n",
    );
    assert_eq!(
        Machine::of(&rig).chip.as_deref(),
        Some("NVIDIA GeForce RTX 3090 + 2× NVIDIA GeForce RTX 4090")
    );
    // A recording made before the identifier was read matches any.
    let unknown = Machine {
        model: None,
        ..machine.clone()
    };
    assert!(unknown.same(&machine) && machine.same(&unknown));
    let laptop = Machine {
        model: Some("Mac16,6".into()),
        ..machine.clone()
    };
    assert!(
        !laptop.same(&machine),
        "a MacBook Pro is not the Mac Studio"
    );
    let more_ram = Machine {
        ram_gb: Some(64),
        ..machine.clone()
    };
    assert!(!more_ram.same(&machine));
}

#[test]
fn speeds_are_kept_for_a_model_its_quantization_runtime_version_and_draft() {
    let sample = serving("Qwen3.8-27B-oQ4e-mtp", "oMLX 0.7.3 · Splash 1.5.0");
    let kept = Identity::of(&sample).unwrap();
    assert_eq!(kept.provider, "oMLX");
    assert_eq!(kept.model, "Qwen3.8-27B-oQ4e-mtp");
    assert_eq!(kept.quantization.as_deref(), Some("4-bit"));
    assert_eq!(kept.runtime.as_deref(), Some("oMLX 0.7.3 · Splash 1.5.0"));
    assert_eq!(kept.draft, Some(false));
    // A model loaded from a path keeps its file name, not the folders.
    for (id, name) in [
        (
            "/Users/someone/models/qwen3-8b-q4_k_m.gguf",
            "qwen3-8b-q4_k_m.gguf",
        ),
        ("~/models/qwen.gguf", "qwen.gguf"),
        (r"C:\Users\someone\models\qwen.gguf", "qwen.gguf"),
        ("mlx-community/Qwen3-8B-4bit", "mlx-community/Qwen3-8B-4bit"),
    ] {
        assert_eq!(Identity::of(&serving(id, "v")).unwrap().model, name);
    }
    // A draft shows by drafting even where the facts do not say.
    let mut plain = serving("m", "v");
    plain.model.draft = None;
    assert_eq!(Identity::of(&plain).unwrap().draft, None);
    plain.llm_tokens_per_pass = Some(3.2);
    assert_eq!(Identity::of(&plain).unwrap().draft, Some(true));
    plain.llm_tokens_per_pass = None;
    plain.model.draft = Some(false);
    assert_eq!(Identity::of(&plain).unwrap().draft, Some(false));
    // Nothing is kept for no model, or for a runtime on another machine.
    assert_eq!(Identity::of(&Sample::default()), None);
    let mut remote = serving("m", "v");
    remote.llm_remote = true;
    assert_eq!(Identity::of(&remote), None);
}

#[test]
fn a_reading_completes_what_was_unknown_and_a_new_version_is_a_change() {
    let kept = identity("oMLX 0.7.3");
    assert_eq!(kept.merged(&kept), (kept.clone(), Change::Same));
    // A reading that leaves something unknown keeps what was known.
    let blank = Identity {
        quantization: None,
        runtime: None,
        draft: None,
        ..kept.clone()
    };
    assert_eq!(kept.merged(&blank), (kept.clone(), Change::Same));
    // What was unknown becomes known: the same work, better named.
    let (completed, change) = blank.merged(&kept);
    assert_eq!((completed, change), (kept.clone(), Change::Completed));
    // A new version under the same name is a change.
    let updated = identity("oMLX 0.8.0");
    assert_eq!(kept.merged(&updated), (updated.clone(), Change::Changed));
    // A draft once seen stays: a runtime shows it only while it drafts.
    let drafting = Identity {
        draft: Some(true),
        ..kept.clone()
    };
    assert_eq!(drafting.merged(&kept).1, Change::Same);
    let (seen, change) = kept.merged(&drafting);
    assert_eq!((seen.draft, change), (Some(true), Change::Changed));
}

#[test]
fn with_a_draft_only_verify_passes_are_kept_and_compared() {
    let dir = TempDir::new("baselines-draft");
    let path = dir.0.join("baselines.json");
    let mut sample = serving("m", "oMLX 0.7.3");
    sample.model.draft = Some(true);
    let drafting = Identity::of(&sample).unwrap();
    let passes = Kind {
        per: Per::Pass,
        ..KIND
    };
    write(
        &path,
        &[
            entry(&drafting, vec![58.0], now_seconds()),
            Entry {
                per: Per::Pass,
                ..entry(&drafting, vec![15.8], now_seconds())
            },
        ],
    )
    .unwrap();
    // Tokens a second with a draft are also this session's text: neither
    // judged against nor kept. Verify passes are.
    let mut baselines = Baselines::new(path.clone(), true);
    let Follow::Seed(seeds) = baselines.follow(&sample) else {
        panic!("seeded");
    };
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].kind, passes);
    baselines.learned(KIND, 58.0, None);
    assert!(baselines.pending.is_empty());
    baselines.learned(passes, 15.8, None);
    assert_eq!(baselines.pending.len(), 1);
    // Without a draft, tokens a second are the machine's.
    assert!(identity("v").compares(Per::Token));
    assert!(!drafting.compares(Per::Token) && drafting.compares(Per::Pass));
}

#[test]
fn bands_of_context_take_their_speeds_at_their_middle() {
    for (context, band, depth) in [
        (None, Band::Unknown, None),
        (Some(0), Band::Under1k, Some(512)),
        (Some(1_023), Band::Under1k, Some(512)),
        (Some(1_024), Band::To4k, Some(2_048)),
        (Some(16_383), Band::To16k, Some(8_192)),
        (Some(40_000), Band::To64k, Some(32_768)),
        (Some(65_536), Band::From64k, Some(131_072)),
        (Some(500_000), Band::From64k, Some(131_072)),
    ] {
        assert_eq!(Band::of(context), band, "{context:?}");
        assert_eq!(band.depth(), depth);
    }
}

#[test]
fn the_file_is_written_whole_and_private() {
    let dir = TempDir::new("baselines-private");
    let path = dir.0.join("state/mlxtop/baselines.json");
    let kept = identity("oMLX 0.7.3");
    let entries = vec![entry(&kept, vec![58.0, 57.5, 58.25], 1_000)];
    assert!(write(&path, &entries).unwrap(), "the first write makes it");
    assert!(!write(&path, &entries).unwrap(), "the second replaces it");
    assert_eq!(read(&path), Read::Entries(entries));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "readable by its owner only");
        assert_eq!(mode(path.parent().unwrap()), 0o700, "in a private folder");
        // A folder made wider before (the log's) is made private.
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        write(&path, &[]).unwrap();
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }
    // Nothing is left beside it.
    let names: Vec<String> = fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["baselines.json"]);
    // A write that cannot finish leaves the old file and no temporary.
    let blocked = dir.0.join("blocked");
    fs::write(&blocked, "a file where the folder should be").unwrap();
    assert!(write(&blocked.join("baselines.json"), &[]).is_err());
}

#[test]
fn the_file_holds_numbers_and_names_never_what_was_asked() {
    let dir = TempDir::new("baselines-privacy");
    let path = dir.0.join("baselines.json");
    let mut sample = serving("/Users/someone/models/qwen.gguf", "llama.cpp b6123");
    sample.llm_requests = vec![crate::domain::RequestUsage {
        id: "chatcmpl-secret-request-id".into(),
        prompt: 30_000,
        ..Default::default()
    }];
    sample.largest_consumer = Some("Secret App".into());
    sample.llm_processes = vec![crate::domain::LlmProcess {
        pid: 4242,
        name: "llama-server".into(),
        command: "llama-server --api-key hunter2 -m /Users/someone/models/qwen.gguf".into(),
        rss: 1 << 30,
        cpu: 99.0,
        memory_percent: None,
        state: "R".into(),
        pageins: None,
        pagein_rate: None,
        footprint: None,
    }];
    let mut baselines = Baselines::new(path.clone(), true);
    assert!(matches!(baselines.follow(&sample), Follow::Seed(seeds) if seeds.is_empty()));
    baselines.learned(KIND, 58.0, None);
    baselines.flush();
    let text = fs::read_to_string(&path).unwrap();
    for secret in [
        "secret-request-id",
        "Secret App",
        "hunter2",
        "--api-key",
        "llama-server --",
        "/Users/someone",
    ] {
        assert!(!text.contains(secret), "{secret} reached the file: {text}");
    }
    assert!(text.contains("\"model\":\"qwen.gguf\""), "{text}");
    assert!(text.contains("\"context\":\"16k–64k\""), "{text}");
    assert!(text.starts_with("{\"schema\":1,"), "{text}");
}

#[test]
fn fifty_kinds_of_work_fit_in_about_150_kb() {
    let dir = TempDir::new("baselines-size");
    let path = dir.0.join("baselines.json");
    let kept = identity("oMLX 0.7.3 · Splash 1.5.0");
    // Fifty full windows of speeds as they are kept, to the hundredth.
    let entries: Vec<Entry> = (0..50u64)
        .map(|index| Entry {
            requests: 1 + index % 4,
            model: format!("{}-{index}", kept.model),
            ..entry(
                &kept,
                (0..KEPT)
                    .map(|i| ((123.0 + (i % 17) as f64 * 0.37) * 100.0).round() / 100.0)
                    .collect(),
                now_seconds(),
            )
        })
        .collect();
    write(&path, &entries).unwrap();
    let size = fs::metadata(&path).unwrap().len();
    assert!(size <= 150 * 1024, "{size} bytes for 50 kinds");
    // The most kinds kept, the least recently seen going first.
    let many: Vec<Entry> = (0..MAX_ENTRIES as u64 + 10)
        .map(|index| Entry {
            model: format!("m{index}"),
            ..entry(&kept, vec![1.0], index)
        })
        .collect();
    let cleaned = clean(many);
    assert_eq!(cleaned.len(), MAX_ENTRIES);
    assert!(cleaned.iter().all(|entry| entry.seen >= 10));
    // A window keeps its latest speeds; nothing that is no speed.
    let long = clean(vec![entry(
        &kept,
        (0..KEPT as u64 + 5)
            .map(|i| i as f64)
            .chain([f64::NAN, -1.0, 0.0])
            .collect(),
        1,
    )]);
    assert_eq!(long[0].speeds.len(), KEPT);
    assert_eq!(long[0].speeds[0], 5.0);
    assert!(clean(vec![Entry {
        requests: 9,
        ..entry(&kept, vec![1.0], 1)
    }])
    .is_empty());
}

#[test]
fn a_corrupt_or_newer_file_is_ignored_and_set_aside_before_it_is_replaced() {
    for (label, text, reason) in [
        (
            "corrupt",
            "{\"schema\":1,\"baselines\":[{\"oops\"",
            "not JSON",
        ),
        (
            "future",
            "{\"schema\":2,\"baselines\":[]}",
            "schema 2 is newer than 1",
        ),
        ("unnumbered", "{\"baselines\":[]}", "no schema number"),
        (
            "wrong",
            "{\"schema\":1,\"baselines\":[{\"model\":3}]}",
            "not a baseline file",
        ),
    ] {
        let dir = TempDir::new(&format!("baselines-{label}"));
        let path = dir.write("baselines.json", text);
        assert!(
            matches!(read(&path), Read::Unusable(why) if why.contains(reason)),
            "{label}"
        );
        // Ignored: nothing to judge against, and the reason said once.
        let mut baselines = Baselines::new(path.clone(), true);
        let sample = serving("m", "oMLX 0.7.3");
        assert_eq!(baselines.follow(&sample), Follow::Seed(Vec::new()));
        assert!(baselines
            .unusable
            .as_deref()
            .is_some_and(|why| why.contains(reason)));
        // Replaced safely: the old file set aside whole, a good one written.
        baselines.learned(KIND, 58.0, None);
        baselines.flush();
        assert_eq!(
            fs::read_to_string(dir.0.join("baselines.json.unread")).unwrap(),
            text
        );
        assert!(matches!(read(&path), Read::Entries(entries) if entries.len() == 1));
        assert_eq!(baselines.unusable, None);
    }
    // Too large to be one mlxtop wrote.
    let dir = TempDir::new("baselines-large");
    let path = dir.write("baselines.json", &" ".repeat(4 * 1024 * 1024 + 1));
    assert!(matches!(read(&path), Read::Unusable(why) if why.contains("larger")));
    // A file another mlxtop fixed meanwhile is not set aside.
    let dir = TempDir::new("baselines-fixed");
    let path = dir.write("baselines.json", "garbage");
    let mut baselines = Baselines::new(path.clone(), true);
    baselines.follow(&serving("m", "v"));
    write(&path, &[]).unwrap();
    baselines.learned(KIND, 58.0, None);
    baselines.flush();
    assert!(!dir.0.join("baselines.json.unread").exists());
    // Unreadable or missing for a replay: it says why.
    assert!(Baselines::read_only(&dir.0.join("missing.json"))
        .unwrap_err()
        .to_string()
        .contains("no such file"));
    let bad = dir.write("bad.json", "nope");
    assert!(Baselines::read_only(&bad)
        .unwrap_err()
        .to_string()
        .contains("not usable"));
}

#[test]
fn a_model_is_judged_against_its_runtime_version_or_the_one_an_update_replaced() {
    let dir = TempDir::new("baselines-seeds");
    let path = dir.0.join("baselines.json");
    let old = identity("oMLX 0.7.3");
    let other_version = identity("oMLX 0.6.0");
    let now = now_seconds();
    let mut requantized = entry(&old, vec![70.0], now);
    requantized.quantization = Some("8-bit".into());
    let mut elsewhere = entry(&old, vec![90.0], now);
    elsewhere.machine.chip = Some("Apple M5 Max".into());
    let mut two = entry(&old, vec![45.0], now);
    two.requests = 2;
    let mut expired = entry(&old, vec![99.0], 1_000);
    expired.context = Band::To4k;
    write(
        &path,
        &[
            entry(&other_version, vec![50.0], now - 100),
            entry(&old, vec![58.0], now - 10),
            requantized,
            elsewhere,
            two,
            expired,
        ],
    )
    .unwrap();
    // The update's first session: the version seen most recently, per
    // kind of work; never another quantization, machine, or a stale kind.
    let mut baselines = Baselines::new(path.clone(), true);
    let Follow::Seed(seeds) = baselines.follow(&serving(&old.model, "oMLX 0.8.0")) else {
        panic!("a model first seen is seeded");
    };
    assert_eq!(
        seeds,
        vec![
            Seed {
                kind: Kind {
                    requests: 2,
                    ..KIND
                },
                speeds: vec![45.0],
                runtime: Some("oMLX 0.7.3".into()),
            },
            Seed {
                kind: KIND,
                speeds: vec![58.0],
                runtime: Some("oMLX 0.7.3".into()),
            },
        ]
    );
    assert_eq!(baselines.runtime(), Some("oMLX 0.8.0"));
    // A replay reads stale kinds too: the recording may be as old.
    let mut replay = Baselines::read_only(&path).unwrap();
    let Follow::Seed(seeds) = replay.follow(&serving(&old.model, "oMLX 0.8.0")) else {
        panic!("seeded");
    };
    assert!(seeds.iter().any(|seed| seed.speeds == vec![99.0]));
    // Its own version's speeds win once it has some; a save drops the
    // stale kind.
    baselines.learned(KIND, 40.0, None);
    baselines.flush();
    let mut next = Baselines::new(path.clone(), true);
    let Follow::Seed(seeds) = next.follow(&serving(&old.model, "oMLX 0.8.0")) else {
        panic!("seeded");
    };
    assert_eq!(seeds[0].speeds, vec![40.0]);
    assert_eq!(seeds[0].runtime.as_deref(), Some("oMLX 0.8.0"));
    let Read::Entries(entries) = read(&path) else {
        panic!("readable");
    };
    assert!(entries.iter().all(|entry| entry.speeds != vec![99.0]));
}

#[test]
fn what_the_identity_learns_decides_seeding_and_restarts() {
    let dir = TempDir::new("baselines-follow");
    let path = dir.0.join("baselines.json");
    write(
        &path,
        &[entry(&identity("oMLX 0.7.3"), vec![58.0], now_seconds())],
    )
    .unwrap();
    let mut baselines = Baselines::new(path.clone(), true);
    // Off: nothing is read, learned or written.
    baselines.on = false;
    assert_eq!(
        baselines.follow(&serving("Qwen3.8-27B-oQ4e-mtp", "oMLX 0.7.3")),
        Follow::Same
    );
    baselines.learned(KIND, 1.0, None);
    assert!(baselines.pending.is_empty());
    baselines.on = true;
    // The version not yet known: seeded from the version seen last.
    let mut unknown = serving("Qwen3.8-27B-oQ4e-mtp", "x");
    unknown.model.runtime_version = None;
    assert!(matches!(baselines.follow(&unknown), Follow::Seed(seeds) if seeds.len() == 1));
    assert_eq!(baselines.follow(&unknown), Follow::Same);
    // Then known: seeded again, nothing learned lost.
    baselines.learned(KIND, 57.0, None);
    let same = serving("Qwen3.8-27B-oQ4e-mtp", "oMLX 0.7.3");
    assert!(matches!(baselines.follow(&same), Follow::Seed(_)));
    assert_eq!(baselines.pending.len(), 1);
    // Updated under the same name: saved as the old version's, restarted.
    let updated = serving("Qwen3.8-27B-oQ4e-mtp", "oMLX 0.8.0");
    assert!(
        matches!(baselines.follow(&updated), Follow::Restart(seeds) if seeds[0].runtime.as_deref() == Some("oMLX 0.7.3"))
    );
    assert!(baselines.pending.is_empty());
    let Read::Entries(entries) = read(&path) else {
        panic!("readable");
    };
    assert_eq!(entries[0].speeds, vec![58.0, 57.0]);
    // Another model: what was learned is saved first, under the last one.
    baselines.learned(KIND, 39.0, None);
    assert!(
        matches!(baselines.follow(&serving("other", "oMLX 0.8.0")), Follow::Seed(seeds) if seeds.is_empty())
    );
    let Read::Entries(entries) = read(&path) else {
        panic!("readable");
    };
    assert!(entries
        .iter()
        .any(|entry| entry.runtime.as_deref() == Some("oMLX 0.8.0") && entry.speeds == vec![39.0]));
    // A restart saves and forgets the model, seen anew next time.
    baselines.learned(KIND, 41.0, None);
    baselines.restart();
    assert!(baselines.pending.is_empty() && baselines.identity.is_none());
    // No local model: nothing to follow.
    assert_eq!(baselines.follow(&Sample::default()), Follow::Same);
}

#[test]
fn speeds_are_saved_at_once_the_first_time_then_every_five_minutes() {
    let dir = TempDir::new("baselines-period");
    let path = dir.0.join("baselines.json");
    let mut baselines = Baselines::new(path.clone(), true);
    baselines.follow(&serving("m", "oMLX 0.7.3"));
    let at = |seconds: u64| Some(UNIX_EPOCH + Duration::from_secs(1_000_000 + seconds));
    // Nothing learned, nothing written.
    baselines.save_if_due(at(0));
    assert!(!path.exists());
    // The first speed makes the file at once, and its notice.
    baselines.learned(KIND, 58.0, at(1));
    baselines.save_if_due(at(2));
    assert!(path.exists());
    let notice = baselines.notice.take().expect("the first save says so");
    assert!(notice.starts_with("usual speeds now kept in "), "{notice}");
    assert!(
        notice.ends_with("baselines.json · Settings (4) turns this off"),
        "{notice}"
    );
    // Then every five minutes of sample time.
    baselines.learned(KIND, 57.0, at(10));
    baselines.save_if_due(at(200));
    assert_eq!(baselines.pending.len(), 1);
    baselines.save_if_due(at(310));
    assert!(baselines.pending.is_empty());
    assert_eq!(baselines.notice, None, "the notice is said once");
    let Read::Entries(entries) = read(&path) else {
        panic!("readable");
    };
    assert_eq!(entries[0].speeds, vec![58.0, 57.0]);
    // A save that fails is tried again a period later, its speeds kept.
    let blocked = dir.0.join("blocked");
    fs::write(&blocked, "").unwrap();
    let mut failing = Baselines::new(blocked.join("baselines.json"), true);
    failing.follow(&serving("m", "oMLX 0.7.3"));
    failing.learned(KIND, 58.0, at(0));
    failing.save_if_due(at(1));
    assert!(failing.failing && failing.pending.len() == 1);
    assert_eq!(failing.since, at(1));
    // Once the disk takes it again, what waited is saved.
    fs::remove_file(&blocked).unwrap();
    failing.save_if_due(at(400));
    assert!(!failing.failing && failing.pending.is_empty());
    fs::remove_dir_all(&blocked).unwrap();
    fs::write(&blocked, "").unwrap();
    // What waits for a save is bounded.
    for _ in 0..PENDING_LIMIT + 5 {
        failing.learned(KIND, 1.0, at(2));
    }
    assert_eq!(failing.pending.len(), PENDING_LIMIT);
    // A replay reads and never writes.
    write(&path, &[]).unwrap();
    let mut replay = Baselines::read_only(&path).unwrap();
    replay.follow(&serving("m", "oMLX 0.7.3"));
    replay.learned(KIND, 58.0, at(0));
    replay.flush();
    assert_eq!(read(&path), Read::Entries(Vec::new()));
    // Speeds learned with no model being followed are not kept.
    let mut orphan = Baselines::new(dir.0.join("orphan.json"), true);
    orphan.learned(KIND, 58.0, at(0));
    orphan.flush();
    assert!(orphan.pending.is_empty() && !dir.0.join("orphan.json").exists());
}

#[test]
fn kinds_not_seen_for_180_days_are_forgotten_and_forgetting_is_per_machine() {
    let dir = TempDir::new("baselines-expiry");
    let path = dir.0.join("baselines.json");
    let kept = identity("oMLX 0.7.3");
    let now = now_seconds();
    let old = now - EXPIRY.as_secs() - 60;
    let mut stale = entry(&kept, vec![10.0], old);
    stale.model = "retired".into();
    let mut other_machine = entry(&kept, vec![20.0], now);
    other_machine.machine.chip = Some("Apple M2 Ultra".into());
    write(&path, &[stale, other_machine.clone()]).unwrap();
    let mut baselines = Baselines::new(path.clone(), true);
    baselines.follow(&serving(&kept.model, "oMLX 0.7.3"));
    baselines.learned(KIND, 58.0, None);
    baselines.flush();
    let Read::Entries(entries) = read(&path) else {
        panic!("readable");
    };
    assert_eq!(entries.len(), 2, "the stale kind went");
    assert!(entries.iter().all(|entry| entry.model != "retired"));
    // Forgetting this machine's speeds keeps another machine's.
    baselines.learned(KIND, 57.0, None);
    baselines.forget(&kept.machine);
    assert!(baselines.pending.is_empty());
    assert_eq!(read(&path), Read::Entries(vec![other_machine]));
    // Off, forgetting leaves the file alone.
    baselines.on = false;
    baselines.forget(&Machine::default());
    assert!(matches!(read(&path), Read::Entries(entries) if entries.len() == 1));
}

#[test]
fn the_finding_names_the_update_between_the_sessions() {
    let below = |percent, then: Option<&str>, now: Option<&str>| BelowEarlier {
        percent,
        then: then.map(str::to_owned),
        now: now.map(str::to_owned),
    };
    let updated = below(31.03, Some("oMLX 0.7.3"), Some("oMLX 0.8.0"));
    assert_eq!(
        updated.sentence(),
        "This model now generates 31% below its usual from earlier sessions at this depth, since oMLX 0.7.3 became 0.8.0"
    );
    assert_eq!(updated.update(), Some(("oMLX 0.7.3", "oMLX 0.8.0")));
    // The same version, or one not known then or now: no update to name.
    for (then, now) in [
        (Some("oMLX 0.8.0"), Some("oMLX 0.8.0")),
        (None, Some("oMLX 0.8.0")),
        (Some("oMLX 0.7.3"), None),
    ] {
        let same = below(24.6, then, now);
        assert_eq!(
            same.sentence(),
            "This model now generates 25% below its usual from earlier sessions at this depth"
        );
        assert_eq!(same.update(), None);
    }
    for (old, new, change) in [
        (
            "oMLX 0.7.3 · Splash 1.5.0",
            "oMLX 0.7.3 · Splash 1.6.1",
            "Splash 1.5.0 became 1.6.1",
        ),
        (
            "llama.cpp b6000",
            "llama.cpp b6123",
            "llama.cpp b6000 became b6123",
        ),
        (
            "oMLX 0.7.3 · Splash 1.5.0",
            "oMLX 0.8.0 · Splash 1.6.1",
            "oMLX 0.7.3 · Splash 1.5.0 became oMLX 0.8.0 · Splash 1.6.1",
        ),
        (
            "oMLX 0.7.3 · Splash 1.5.0",
            "oMLX 0.7.3",
            "oMLX 0.7.3 · Splash 1.5.0 became oMLX 0.7.3",
        ),
        ("oMLX", "oMLX 0.8.0", "oMLX became oMLX 0.8.0"),
        (
            "oMLX 0.7.3",
            "Ollama 0.12.0",
            "oMLX 0.7.3 became Ollama 0.12.0",
        ),
    ] {
        assert_eq!(version_change(old, new), change);
    }
}

#[test]
fn the_collector_keeps_forgets_and_tells_of_the_file() {
    let dir = TempDir::new("baselines-collector");
    let path = dir.0.join("baselines.json");
    let host = FakeHost::default().command("/usr/sbin/sysctl -n hw.model", "Mac16,9\n");
    let mut collector = Collector::with_host(
        40,
        crate::test_support::offline_config(),
        Box::new(host),
        Platform::MacOs,
        None,
    );
    collector.keep_baselines(Some(path.clone()), true);
    assert_eq!(collector.metal.machine_model.as_deref(), Some("Mac16,9"));
    // Settings turns it off and on; off drops what was not saved.
    collector
        .correlation
        .baselines
        .as_deref_mut()
        .unwrap()
        .follow(&serving("m", "v"));
    collector
        .correlation
        .baselines
        .as_deref_mut()
        .unwrap()
        .learned(KIND, 1.0, None);
    let off = Config {
        baselines: Some(false),
        ..Config::default()
    };
    collector.apply_baselines(&off);
    collector.apply_baselines(&off);
    let baselines = collector.correlation.baselines.as_deref().unwrap();
    assert!(!baselines.on && baselines.pending.is_empty() && baselines.identity.is_none());
    collector.apply_baselines(&Config::default());
    assert!(collector.correlation.baselines.as_deref().unwrap().on);
    // The notice goes in the Journal once, as a system row.
    let baselines = collector.correlation.baselines.as_deref_mut().unwrap();
    baselines.follow(&serving("m", "v"));
    baselines.learned(KIND, 58.0, None);
    collector.save_baselines();
    collector.view.current.updated = "12:00:01".into();
    for index in 0..40 {
        collector.view.signals.push_back(SignalEvent {
            time: format!("11:00:{index:02}"),
            recorded_at: SystemTime::now(),
            kind: EventKind::Llm,
            state: "PROMPT".into(),
            summary: format!("request {index}"),
            tone: Tone::Cyan,
        });
    }
    collector.journal_baselines();
    collector.journal_baselines();
    assert_eq!(collector.signals.len(), 40, "the Journal keeps its length");
    let rows: Vec<&SignalEvent> = collector
        .signals
        .iter()
        .filter(|event| event.summary.contains("usual speeds"))
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "SYSTEM");
    assert_eq!(rows[0].kind, EventKind::System);
    assert_eq!(rows[0].time, "12:00:01");
    // Forgetting clears this machine's speeds from the file.
    collector.view.current = serving("m", "v");
    collector.forget_baselines();
    assert_eq!(read(&path), Read::Entries(Vec::new()));
    // Without a home there is nothing kept, and nothing to forget.
    collector.keep_baselines(None, true);
    collector.apply_baselines(&off);
    collector.forget_baselines();
    collector.save_baselines();
    collector.journal_baselines();
    assert!(collector.correlation.baselines.is_none());
}

#[test]
fn the_sampler_forgets_kept_speeds_on_f_and_saves_when_it_stops() {
    let dir = TempDir::new("baselines-sampler");
    let path = dir.0.join("baselines.json");
    let kept_in = path.clone();
    let mut sampler = crate::sampler::Sampler::start(Duration::from_millis(5), move || {
        let mut collector = Collector::with_host(
            40,
            crate::test_support::offline_config(),
            Box::new(FakeHost::default()),
            Platform::Linux,
            None,
        );
        collector.keep_baselines(Some(kept_in), true);
        collector
    });
    let view = sampler
        .views
        .recv_timeout(Duration::from_secs(10))
        .expect("a first view");
    // This machine's speeds, and another machine's.
    let here = identity("oMLX 0.7.3");
    let mut mine = entry(&here, vec![58.0], now_seconds());
    mine.machine = Machine::of(&view.current);
    let mut other = entry(&here, vec![70.0], now_seconds());
    other.machine.chip = Some("Apple M2 Ultra".into());
    write(&path, &[mine, other.clone()]).unwrap();
    // Commands are taken in order: Settings, the forget, then the stop.
    sampler.send(crate::sampler::SamplerCommand::SetConfig(Box::default()));
    sampler.send(crate::sampler::SamplerCommand::Forget);
    sampler.send(crate::sampler::SamplerCommand::Stop);
    sampler.handle.take().unwrap().join().unwrap();
    assert_eq!(read(&path), Read::Entries(vec![other]));
}

#[test]
fn without_a_screen_the_notice_goes_to_standard_error() {
    // A collector whose first save has just made the file.
    let noticed = || {
        let mut collector = Collector::with_host(
            40,
            crate::test_support::offline_config(),
            Box::new(FakeHost::default()),
            Platform::Linux,
            None,
        );
        collector.keep_baselines(Some(PathBuf::from("/nonexistent/baselines.json")), true);
        collector
            .correlation
            .baselines
            .as_deref_mut()
            .unwrap()
            .notice = Some("usual speeds now kept".into());
        collector
    };
    // `mlxtop stream`: the JSON on standard output, the notice beside it.
    let mut collector = noticed();
    let lines = std::sync::atomic::AtomicUsize::new(0);
    let stop = || lines.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 1;
    let mut out = Vec::new();
    crate::export::stream(&mut collector, Duration::ZERO, &mut out, &stop).unwrap();
    assert_eq!(collector.baselines_notice(), None, "said once");
    assert!(!String::from_utf8(out).unwrap().contains("usual speeds"));
    // `mlxtop serve`: no Journal is read.
    let latest = crate::exporter::Latest::default();
    let sampler =
        crate::sampler::Sampler::headless(Duration::from_millis(5), noticed, latest.clone());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while latest.get().is_none() {
        assert!(std::time::Instant::now() < deadline, "no sample");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(sampler);
}
