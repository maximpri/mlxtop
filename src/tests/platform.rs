use crate::test_support::*;
// SPDX-License-Identifier: MIT
// macOS thermal readings through the host seam.
use super::*;

/// What an Intel Mac's pmset prints once its CPU power status is recorded.
const PMSET_INTEL: &str = "\
Note: No thermal warning level has been recorded\n\
Note: No performance warning level has been recorded\n\
2026-11-15 02:00:00 -0500 CPU Power notify\n\
\tCPU_Scheduler_Limit \t= 100\n\
\tCPU_Available_CPUs \t= 8\n\
\tCPU_Speed_Limit \t= 70\n";

const NOTIFYUTIL: &str = "/usr/bin/notifyutil -g com.apple.system.thermalpressurelevel";
const PMSET: &str = "/usr/bin/pmset -g therm";

fn state(text: &str) -> String {
    format!("{THERMAL_PRESSURE_KEY} {text}\n")
}

#[test]
fn the_thermal_pressure_level_reads_in_both_numbering_schemes() {
    use ThermalLevel::{Heavy, Moderate, Nominal, Sleeping, Trapping};
    assert_eq!(parse_thermal_pressure(NOTIFYUTIL_THERMAL), Some(Nominal));
    // macOS numbers the levels 0–4; Apple's other systems 0, 10 … 50, with
    // a Light step at 10 below Moderate.
    for (states, level) in [
        (["0", "10"], Nominal),
        (["1", "20"], Moderate),
        (["2", "30"], Heavy),
        (["3", "40"], Trapping),
        (["4", "50"], Sleeping),
    ] {
        for text in states {
            assert_eq!(parse_thermal_pressure(&state(text)), Some(level), "{text}");
        }
    }
}

#[test]
fn an_unreadable_thermal_pressure_level_is_unknown_never_nominal() {
    for text in [
        String::new(),
        THERMAL_PRESSURE_KEY.to_owned(),
        // Numbers neither scheme defines.
        state("5"),
        state("60"),
        state("-1"),
        state("18446744073709551615"),
        state("nominal"),
        state("0 1"),
        // What notifyutil prints when the read fails.
        format!("{THERMAL_PRESSURE_KEY}: Failed with code 1\n"),
        // Another key, and one that only starts like it.
        "com.apple.system.lowpowermode 0\n".to_owned(),
        format!("{THERMAL_PRESSURE_KEY}2 0\n"),
    ] {
        assert_eq!(parse_thermal_pressure(&text), None, "{text:?}");
    }
}

#[test]
fn pmset_gives_a_cpu_speed_limit_on_intel_and_none_on_apple_silicon() {
    assert_eq!(parse_cpu_speed_limit(PMSET_INTEL), Some(70));
    assert_eq!(
        parse_cpu_speed_limit(&PMSET_INTEL.replace("= 70", "= 100")),
        Some(100)
    );
    for text in [
        PMSET_APPLE_SILICON,
        "",
        "Thermal warning level set to 2",
        "CPU_Speed_Limit = 250",
    ] {
        assert_eq!(parse_cpu_speed_limit(text), None, "{text:?}");
    }
}

#[test]
fn apple_silicon_reads_the_level_and_runs_pmset_only_when_it_cannot() {
    let host = FakeHost::default()
        .command(NOTIFYUTIL, NOTIFYUTIL_THERMAL)
        .command(PMSET, PMSET_APPLE_SILICON);
    let thermal = macos_thermal(&host, Some("arm64"));
    assert_eq!(thermal.level, Some(ThermalLevel::Nominal));
    assert_eq!(thermal.cpu_speed_limit, None);
    assert_eq!(
        (thermal.label().as_str(), thermal.limited()),
        ("nominal", false)
    );
    assert_eq!(host.calls(), vec![NOTIFYUTIL.to_owned()]);

    // notifyutil missing, failing or printing something else: unknown, not
    // nominal. pmset, read in its place, has no limit to give here.
    for notifyutil in [
        None,
        Some(format!("{THERMAL_PRESSURE_KEY}: Failed with code 1\n")),
        Some("usage: notifyutil [-q] [-v]\n".to_owned()),
    ] {
        let mut host = FakeHost::default().command(PMSET, PMSET_APPLE_SILICON);
        if let Some(text) = &notifyutil {
            host = host.command(NOTIFYUTIL, text);
        }
        let thermal = macos_thermal(&host, Some("arm64"));
        assert_eq!(thermal, ThermalState::default(), "{notifyutil:?}");
        assert_eq!(thermal.rank(), None);
        assert_eq!(thermal.label(), "unavailable");
        assert!(!thermal.limited());
        assert!(host.calls().contains(&PMSET.to_owned()));
    }
}

#[test]
fn intel_reads_pmsets_cpu_speed_limit_beside_the_level() {
    let host = FakeHost::default()
        .command(NOTIFYUTIL, NOTIFYUTIL_THERMAL)
        .command(PMSET, PMSET_INTEL);
    let thermal = macos_thermal(&host, Some("x86_64"));
    assert_eq!(thermal.level, Some(ThermalLevel::Nominal));
    assert_eq!(thermal.cpu_speed_limit, Some(70));
    assert_eq!(thermal.label(), "limited 70%");
    assert!(thermal.limited());
    // An unknown architecture reads pmset too.
    let host = FakeHost::default().command(PMSET, &PMSET_INTEL.replace("= 70", "= 100"));
    let thermal = macos_thermal(&host, None);
    assert_eq!((thermal.level, thermal.cpu_speed_limit), (None, Some(100)));
    // Full speed is a reading, and not a limit.
    assert_eq!(thermal.rank(), Some(0));
    assert_eq!(thermal.label(), "no limit");
    assert!(!thermal.limited());
}

#[test]
fn the_thermal_state_ranks_says_and_explains_what_macos_reported() {
    use ThermalLevel::{Heavy, Moderate, Nominal, Sleeping, Trapping};
    let thermal = |level, cpu_speed_limit| ThermalState {
        level,
        cpu_speed_limit,
    };
    // The state, its rank, its words, and its words as a slowdown's reason.
    for (state, rank, words, reason) in [
        (thermal(None, None), None, "unavailable", "thermal limit"),
        (
            thermal(Some(Nominal), None),
            Some(0),
            "nominal",
            "thermal limit",
        ),
        (
            thermal(Some(Moderate), None),
            Some(1),
            "moderate",
            "thermal pressure moderate",
        ),
        (
            thermal(Some(Trapping), None),
            Some(3),
            "trapping",
            "thermal pressure trapping",
        ),
        (
            thermal(Some(Sleeping), Some(100)),
            Some(4),
            "sleeping",
            "thermal pressure sleeping",
        ),
        (
            thermal(None, Some(60)),
            Some(1),
            "limited 60%",
            "CPU speed limit 60%",
        ),
        (
            thermal(Some(Heavy), Some(60)),
            Some(2),
            "heavy, limited 60%",
            "thermal pressure heavy, CPU speed limit 60%",
        ),
    ] {
        assert_eq!(state.rank(), rank, "{state:?}");
        assert_eq!(state.limited(), rank.is_some_and(|rank| rank > 0));
        assert_eq!(state.label(), words, "{state:?}");
        assert_eq!(state.reason(), reason, "{state:?}");
    }
}

#[test]
fn a_mac_sample_records_the_level_and_older_recordings_still_replay() {
    let mut sample = Sample::default();
    sample.metal.architecture = Some("arm64".into());
    let host = FakeHost::default().command(NOTIFYUTIL, &state("2"));
    sample_macos_gpu_thermal(&host, &mut sample, true);
    assert_eq!(sample.thermal_state.level, Some(ThermalLevel::Heavy));
    assert_eq!(sample.thermal, "heavy");
    assert!(sample.thermal_news());

    // Recorded by name, and read back.
    let line = serde_json::to_string(&sample).unwrap();
    assert!(
        line.contains(
            r#""thermal":"heavy","thermal_state":{"level":"Heavy","cpu_speed_limit":null}"#
        ),
        "{line}"
    );
    let (samples, skipped) = crate::recording::read_counted(line.as_bytes()).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(samples[0].thermal_state, sample.thermal_state);

    // A recording from before the level was read keeps its words, and its
    // level is unknown: not nominal, and not a limit.
    let old = r#"{"updated":"12:00:01","thermal":"no warning"}"#;
    let (samples, skipped) = crate::recording::read_counted(old.as_bytes()).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(samples[0].thermal, "no warning");
    assert_eq!(samples[0].thermal_state, ThermalState::default());
    assert!(!samples[0].thermal_news());
}
