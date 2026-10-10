// SPDX-License-Identifier: MIT
//! Process detection, grouping and per-process page-in rates.
use crate::domain::{Consumer, LlmProcess, ProcessSnapshot};
use crate::history::delta;
use std::time::Duration;
/// One process as reported by `ps` or the Windows process list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProcessRow {
    pub pid: u32,
    /// Resident memory in bytes.
    pub rss: u64,
    pub cpu: f64,
    pub memory_percent: Option<f64>,
    pub state: String,
    pub pageins: Option<u64>,
    /// Executable name without its directory.
    pub name: String,
    pub command: String,
}

pub(crate) fn parse_processes(text: &str) -> ProcessSnapshot {
    snapshot(text.lines().filter_map(parse_ps_line).collect())
}

fn parse_ps_line(line: &str) -> Option<ProcessRow> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 4 {
        return None;
    }
    let pid = fields[0].parse::<u32>().ok()?;
    let rss_kib = fields[1].parse::<u64>().ok()?;
    let cpu = fields[2].parse::<f64>().ok()?;
    let modern_memory_percent = fields
        .get(3)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0);
    let (memory_percent, state, pageins, name_index, command_index) =
        if modern_memory_percent.is_some() {
            (
                modern_memory_percent,
                fields.get(4).copied().unwrap_or("?").to_string(),
                fields.get(5).and_then(|value| value.parse::<u64>().ok()),
                6,
                7,
            )
        } else {
            (None, "?".into(), None, 3, 4)
        };
    let name_field = fields.get(name_index)?;
    let name = name_field
        .rsplit('/')
        .next()
        .unwrap_or(name_field)
        .to_string();
    let command = fields
        .get(command_index..)
        .map(|parts| parts.join(" "))
        .unwrap_or_else(|| name.clone());
    Some(ProcessRow {
        pid,
        rss: rss_kib * 1024,
        cpu,
        memory_percent,
        state,
        pageins,
        name,
        command,
    })
}

pub(crate) fn snapshot(rows: Vec<ProcessRow>) -> ProcessSnapshot {
    let mut llm_count = 0;
    let mut llm_rss = 0;
    let mut llm_cpu = 0.0;
    let mut provider: Option<String> = None;
    let mut consumers: Vec<Consumer> = Vec::new();
    let mut llm_processes: Vec<LlmProcess> = Vec::new();

    for row in rows {
        let ProcessRow {
            pid,
            rss,
            cpu,
            memory_percent,
            state,
            pageins,
            name,
            command,
        } = row;
        let lower = format!("{name} {command}").to_ascii_lowercase();
        if let Some(detected) = process_provider(&name, &command) {
            provider.get_or_insert(detected);
            llm_count += 1;
            llm_rss += rss;
            llm_cpu += cpu;
            llm_processes.push(LlmProcess {
                pid,
                name: name.clone(),
                command,
                rss,
                cpu,
                memory_percent,
                state,
                pageins,
                pagein_rate: None,
            });
        }

        if lower.contains("mlxtop") || name == "ps" || name == "awk" {
            continue;
        }
        if let Some(consumer) = consumers.iter_mut().find(|c| c.name == name) {
            consumer.rss += rss;
            consumer.processes += 1;
        } else {
            consumers.push(Consumer {
                name,
                rss,
                processes: 1,
            });
        }
    }
    consumers.sort_by_key(|consumer| std::cmp::Reverse(consumer.rss));
    let largest_consumer = consumers.first().map(|consumer| consumer.name.clone());
    llm_processes.sort_by_key(|process| std::cmp::Reverse(process.rss));
    llm_processes.truncate(32);
    let top_llm = llm_processes.first().cloned();
    ProcessSnapshot {
        llm_count,
        llm_rss,
        llm_cpu,
        top_llm,
        provider,
        largest_consumer,
        llm_processes,
    }
}

pub(crate) fn annotate_process_pagein_rates(
    processes: &mut [LlmProcess],
    previous: &[LlmProcess],
    elapsed: Duration,
) {
    let seconds = elapsed.as_secs_f64().max(0.001);
    for process in processes {
        process.pagein_rate = process.pageins.and_then(|current| {
            previous
                .iter()
                .find(|old| old.pid == process.pid)
                .and_then(|old| old.pageins)
                .map(|old| delta(current, old) as f64 / seconds)
        });
    }
}

pub(crate) fn normalize_process_token(value: &str) -> String {
    value
        .trim_matches(['"', '\''])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(value)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn process_provider(name: &str, command: &str) -> Option<String> {
    let command_tokens = command
        .split_whitespace()
        .take(4)
        .take_while(|token| !token.starts_with("--"))
        .collect::<Vec<_>>();
    let prefix = command_tokens.join(" ").to_ascii_lowercase();
    let candidates: Vec<_> = std::iter::once(name)
        .chain(command_tokens.iter().copied())
        .map(normalize_process_token)
        .filter(|token| !token.contains("mlxtop"))
        .collect();
    let has = |marker: &str| candidates.iter().any(|token| token.contains(marker));
    // Bionic uses LM Studio's runtime but has a different app executable name.
    // Match the executable's bundle path, not unrelated uses of "bionic".
    let is_bionic = std::iter::once(name)
        .chain(command_tokens.first().copied())
        .any(|token| {
            token
                .trim_matches(['"', '\''])
                .to_ascii_lowercase()
                .ends_with("/bionic.app/contents/macos/bionic")
        });
    // LM Studio can host a llama-server worker; retain the owning runtime.
    let provider = if has("lmstudio")
        || has("llmster")
        || prefix.contains("lm studio")
        || prefix.contains(".lmstudio/")
        || is_bionic
    {
        "LM Studio"
    } else if has("omlx") {
        "oMLX"
    } else if has("ollama") {
        "Ollama"
    } else if has("koboldcpp") {
        "KoboldCpp"
    } else if has("localai") {
        "LocalAI"
    } else if has("vllm") {
        "vLLM"
    } else if has("sglang") {
        "SGLang"
    } else if has("gpt4all") {
        "GPT4All"
    } else if candidates
        .iter()
        .any(|token| token == "jan" || token == "janexe")
        || prefix.contains("/jan.app/")
        || prefix.contains("/.jan/")
    {
        "Jan"
    } else if has("llamaserver") || has("llamacpp") {
        "llama.cpp"
    } else if candidates.iter().any(|token| token == "mlxserve") {
        "mlx-serve"
    } else if has("mlxlm") {
        "mlx-lm"
    } else {
        return None;
    };
    Some(provider.into())
}
