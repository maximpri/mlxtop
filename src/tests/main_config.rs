// SPDX-License-Identifier: MIT
//! Configuration and thresholds.
use super::*;

#[test]
fn config_defaults_when_file_missing() {
    let config = Config::default();
    assert_eq!(config.interval, None);
    assert_eq!(config.history, None);
    assert_eq!(config.omx, None);
    assert_eq!(config.memory_warn_load, None);
    assert_eq!(config.gpu_critical_load, None);
}

#[test]
fn configured_thresholds_replace_the_built_in_defaults() {
    let config: Config = serde_json::from_str(
        r#"{"memory_warn_load":60,"memory_critical_load":75,
            "gpu_warn_load":50,"gpu_critical_load":65,"gpu_warn_exit":40,
            "swap_warn_rate":2097152,"swap_critical_rate":33554432,
            "swap_warn_exit":1048576,
            "compression_warn_rate":134217728,"compression_warn_exit":67108864}"#,
    )
    .expect("valid json should parse");
    let thresholds = Thresholds::from_config(&config);

    assert_eq!(
        thresholds,
        Thresholds {
            memory_warn_load: 60,
            memory_critical_load: 75,
            gpu_warn_load: 50,
            gpu_critical_load: 65,
            gpu_warn_exit: 40,
            swap_warn_rate: 2 * MIB,
            swap_critical_rate: 32 * MIB,
            swap_warn_exit: MIB,
            compression_warn_rate: 128 * MIB,
            compression_warn_exit: 64 * MIB,
        }
    );
    assert_eq!(Thresholds::from_config(&Config::default()), defaults());
}

#[test]
fn inverted_or_out_of_range_thresholds_are_clamped_into_usable_bands() {
    let config: Config = serde_json::from_str(
        r#"{"memory_warn_load":90,"memory_critical_load":40,
            "gpu_warn_load":250,"gpu_critical_load":10,
            "swap_warn_rate":1000,"swap_critical_rate":10,
            "compression_warn_rate":100,"compression_warn_exit":900}"#,
    )
    .expect("valid json should parse");
    let thresholds = Thresholds::from_config(&config);

    assert_eq!(thresholds.memory_warn_load, 90);
    assert_eq!(thresholds.memory_critical_load, 90);
    assert_eq!(thresholds.gpu_warn_load, 100);
    assert_eq!(thresholds.gpu_critical_load, 100);
    assert_eq!(thresholds.swap_warn_rate, 1000);
    assert_eq!(thresholds.swap_critical_rate, 1000);
    assert_eq!(thresholds.compression_warn_exit, 100);
    /* The warning band is never silently removed by bad input. */
    assert_eq!(ChartMetric::Memory.tone(89, thresholds), Tone::Green);
    assert_eq!(ChartMetric::Memory.tone(90, thresholds), Tone::Red);
}

#[test]
fn out_of_range_config_values_fall_back_instead_of_refusing_to_start() {
    let valid: Config =
        serde_json::from_str(r#"{"interval":5,"history":500}"#).expect("valid json should parse");
    assert_eq!(config_interval(&valid), (5, false));
    assert_eq!(config_history(&valid), (500, false));

    let out_of_range: Config =
        serde_json::from_str(r#"{"interval":0,"history":5}"#).expect("valid json should parse");
    assert_eq!(config_interval(&out_of_range), (INTERVAL_DEFAULT, true));
    assert_eq!(config_history(&out_of_range), (HISTORY_DEFAULT, true));

    assert_eq!(
        config_interval(&Config::default()),
        (INTERVAL_DEFAULT, false)
    );
    assert_eq!(config_history(&Config::default()), (HISTORY_DEFAULT, false));
}

/**
 * Every field in `Config` must reach the value mlxtop actually runs with.
 *
 * The destructuring below is exhaustive on purpose: adding a field to
 * `Config` stops this test from compiling until the field is named here,
 * and an unused binding fails `clippy -D warnings`, so a new setting
 * cannot be merged without an assertion that something reads it.
 */
#[test]
fn every_config_field_reaches_the_resolved_settings() {
    let config: Config =
        serde_json::from_str(CONFIG_WITH_EVERY_FIELD_SET).expect("the fixture should parse");

    let Config {
        interval,
        history,
        omx,
        memory_warn_load,
        memory_critical_load,
        gpu_warn_load,
        gpu_critical_load,
        swap_warn_rate,
        swap_critical_rate,
        compression_warn_rate,
        swap_warn_exit,
        compression_warn_exit,
        gpu_warn_exit,
        alarms,
        journal,
    } = config.clone();
    let OmxConfig { host, port } = omx.expect("the fixture sets omx");

    assert_eq!(
        config_interval(&config).0,
        interval.expect("the fixture sets interval")
    );
    assert_eq!(
        config_history(&config).0,
        history.expect("the fixture sets history")
    );
    assert_eq!(
        resolve_omlx_endpoint(&DiscoveredEndpoint::default(), &config),
        (
            host.expect("the fixture sets omx.host"),
            port.expect("the fixture sets omx.port")
        )
    );

    let thresholds = Thresholds::from_config(&config);
    assert_eq!(
        thresholds,
        Thresholds {
            memory_warn_load: memory_warn_load.expect("fixture"),
            memory_critical_load: memory_critical_load.expect("fixture"),
            gpu_warn_load: gpu_warn_load.expect("fixture"),
            gpu_critical_load: gpu_critical_load.expect("fixture"),
            gpu_warn_exit: gpu_warn_exit.expect("fixture"),
            swap_warn_rate: swap_warn_rate.expect("fixture"),
            swap_critical_rate: swap_critical_rate.expect("fixture"),
            swap_warn_exit: swap_warn_exit.expect("fixture"),
            compression_warn_rate: compression_warn_rate.expect("fixture"),
            compression_warn_exit: compression_warn_exit.expect("fixture"),
        },
        "a Config field was parsed but never reached Thresholds"
    );

    let (commands, _command_receiver) = mpsc::channel();
    let (_view_sender, views) = mpsc::channel();
    let app = App::with_sampler(
        1,
        60,
        config.clone(),
        Sampler {
            commands,
            views,
            handle: None,
        },
    );
    assert_eq!(app.alarms, alarms.expect("the fixture sets alarms"));

    let journal = journal.expect("the fixture sets journal");
    let JournalConfig {
        prompts,
        llm,
        queue,
        memory,
        paging,
        gpu,
        thermal,
    } = journal.clone();
    for (toggle, state) in [
        (prompts, "PROMPT"),
        (llm, "LLM"),
        (queue, "QUEUE"),
        (memory, "PRESSURE"),
        (paging, "PAGING"),
        (gpu, "GPU"),
        (thermal, "THERMAL"),
    ] {
        assert_eq!(toggle, Some(false));
        assert!(
            !journal_records(&journal, state, false),
            "journal toggle for {state} was parsed but never filters"
        );
    }
}

/**
 * Every field in `Thresholds` must change something the user can see.
 *
 * This is the regression guard for the "deserialized and then ignored"
 * class of bug: each probe reports an observable result — a chart tone, a
 * load label, a classified impact — and the test fails unless configuring
 * the field changes it. Reverting any field to a hard-coded constant
 * fails here even though parsing still succeeds.
 *
 * The destructuring and the `all_fields` array are exhaustive on purpose:
 * a new `Thresholds` field stops the test compiling, and the length
 * assertion then fails until the field is given a probe below.
 */
#[test]
fn every_threshold_field_changes_an_observable_result() {
    struct Probe {
        field: &'static str,
        config: &'static str,
        observe: fn(Thresholds) -> String,
    }

    fn tone_name(tone: Tone) -> String {
        format!("{tone:?}")
    }

    fn impact_after_classify(
        thresholds: Thresholds,
        previous_impact: Option<&str>,
        prepare: fn(&mut Sample),
    ) -> String {
        let mut sample = Sample {
            rate_ready: true,
            total_memory: 32 * 1024 * 1024 * 1024,
            availability: Some(50),
            pressure: "GREEN".into(),
            vm_available: true,
            swap_available: true,
            thermal: "no warning".into(),
            llm_count: 1,
            ..Sample::default()
        };
        prepare(&mut sample);
        let previous = previous_impact.map(|impact| Sample {
            impact: impact.into(),
            ..Sample::default()
        });
        classify(&mut sample, previous.as_ref(), thresholds);
        sample.impact
    }

    let probes = [
        Probe {
            field: "memory_warn_load",
            config: r#"{"memory_warn_load":55}"#,
            observe: |thresholds| tone_name(ChartMetric::Memory.tone(60, thresholds)),
        },
        Probe {
            field: "memory_critical_load",
            config: r#"{"memory_critical_load":95}"#,
            observe: |thresholds| tone_name(ChartMetric::Memory.tone(85, thresholds)),
        },
        Probe {
            field: "gpu_warn_load",
            config: r#"{"gpu_warn_load":70}"#,
            observe: |thresholds| gpu_load_label(Some(72), thresholds).to_owned(),
        },
        Probe {
            field: "gpu_critical_load",
            config: r#"{"gpu_warn_load":70,"gpu_critical_load":85}"#,
            observe: |thresholds| gpu_load_label(Some(88), thresholds).to_owned(),
        },
        Probe {
            field: "gpu_warn_exit",
            config: r#"{"gpu_warn_exit":60}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, Some("GPU BUSY"), |sample| {
                    sample.gpu_util = Some(65);
                })
            },
        },
        Probe {
            field: "swap_warn_rate",
            config: r#"{"swap_warn_rate":4194304}"#,
            observe: |thresholds| tone_name(ChartMetric::Swap.tone(2 * MIB, thresholds)),
        },
        Probe {
            field: "swap_critical_rate",
            config: r#"{"swap_critical_rate":33554432}"#,
            // Critical paging is a finding (both directions at the rate); the
            // paging chart's red follows that finding.
            observe: |thresholds| {
                impact_after_classify(thresholds, None, |sample| {
                    sample.swap_in = 16 * MIB;
                    sample.swap_out = 16 * MIB;
                })
            },
        },
        Probe {
            field: "swap_warn_exit",
            config: r#"{"swap_warn_exit":4194304}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, Some("PAGING ACTIVE"), |sample| {
                    sample.swap_in = 3 * MIB;
                })
            },
        },
        Probe {
            field: "compression_warn_rate",
            config: r#"{"compression_warn_rate":134217728}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, None, |sample| {
                    sample.compress = 64 * MIB;
                })
            },
        },
        Probe {
            field: "compression_warn_exit",
            config: r#"{"compression_warn_exit":50331648}"#,
            observe: |thresholds| {
                impact_after_classify(thresholds, Some("COMPRESSION ACTIVE"), |sample| {
                    sample.compress = 40 * MIB;
                })
            },
        },
    ];

    let Thresholds {
        memory_warn_load,
        memory_critical_load,
        gpu_warn_load,
        gpu_critical_load,
        gpu_warn_exit,
        swap_warn_rate,
        swap_critical_rate,
        swap_warn_exit,
        compression_warn_rate,
        compression_warn_exit,
    } = defaults();
    let all_fields = [
        memory_warn_load,
        memory_critical_load,
        gpu_warn_load,
        gpu_critical_load,
        gpu_warn_exit,
        swap_warn_rate,
        swap_critical_rate,
        swap_warn_exit,
        compression_warn_rate,
        compression_warn_exit,
    ];
    assert_eq!(
        probes.len(),
        all_fields.len(),
        "every field in Thresholds needs a probe proving it changes behaviour"
    );

    for probe in probes {
        let config: Config = serde_json::from_str(probe.config)
            .unwrap_or_else(|error| panic!("{} fixture should parse: {error}", probe.field));
        let configured = (probe.observe)(Thresholds::from_config(&config));
        let built_in = (probe.observe)(defaults());
        assert_ne!(
            configured, built_in,
            "setting {} changed nothing observable — it is parsed but not applied",
            probe.field
        );
    }
}
