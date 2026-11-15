// SPDX-License-Identifier: MIT
//! `mlxtop serve`: sampling runs with no screen, and a listener on the
//! loopback interface answers `/metrics` (Prometheus text), `/snapshot` (the
//! JSON `--once --json` prints) and `/health` from the newest sample. A
//! request never starts a sample or waits for one: it reads the slot the
//! sampler last filled, so a runtime that hangs costs a scrape nothing.
//!
//! It listens on loopback only, and answers only requests that name a
//! loopback host (`localhost`, `127.0.0.1`, `[::1]`), so a web page whose
//! own name a DNS server points at 127.0.0.1 (DNS rebinding) is refused.
//! Requests are answered one at a time: a request head gets 2 seconds and
//! 8 KiB at most, and only GET and HEAD are served, with no CORS headers
//! and no HTML.
//!
//! Metric names follow Prometheus's conventions: an `mlxtop_` prefix, the
//! base unit in the name (`_bytes`, `_seconds`, `_ratio` from 0 to 1,
//! `_celsius`, `_watts`), `_total` on counters, and a label only where a
//! value needs one: the runtime and model in `mlxtop_llm_model_info`, the
//! verdict's state in `mlxtop_bottleneck`, a GPU's index. Request ids and
//! process names never appear. A reading that is unknown is left out, never
//! written as 0. Each value comes from the `Snapshot` the JSON is written
//! from; names, like the JSON schema, are experimental through 3.4.
//!
//! To add a metric, add a row to `READINGS` (or `GPU_READINGS`), then add it
//! to `tests/fixtures/metrics_v1.prom` and to the metric list in
//! docs/USER_GUIDE.md: the tests hold the output to both.
use crate::collector::Collector;
use crate::diagnosis::Verdict;
use crate::export::{Gpu, Snapshot};
use crate::logging::{diagnostics_log, log_field};
use crate::sampler::Sampler;
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Where `mlxtop serve` listens unless told otherwise: loopback, on the
/// first port after the run of exporter ports Prometheus's registry has
/// allocated (10001–10062).
pub(crate) const DEFAULT_LISTEN: SocketAddr =
    SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 10063);

/// The longest a request head may take to arrive, and an answer to leave.
const TIMEOUT: Duration = Duration::from_secs(2);

/// The largest request head read, its blank line included.
const HEAD_LIMIT: usize = 8 * 1024;

/// How often the listener looks for a connection, and `serve` for a stop.
const POLL: Duration = Duration::from_millis(50);

/// The text format Prometheus 3 requires a scrape to name.
pub(crate) const METRICS_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
const TEXT_TYPE: &str = "text/plain; charset=utf-8";
const JSON_TYPE: &str = "application/json";

/// The newest snapshot and when it was taken: the one thing `mlxtop serve`
/// shares between threads. The sampler replaces it after each sample; a
/// request copies the pointer and answers from that, so neither waits on
/// the other for longer than the copy.
#[derive(Clone, Default)]
pub(crate) struct Latest(Arc<Mutex<Option<Arc<Taken>>>>);

/// A sample's snapshot, and when the sampler made it.
pub(crate) struct Taken {
    pub(crate) snapshot: Snapshot,
    pub(crate) at: Instant,
}

impl Latest {
    pub(crate) fn publish(&self, snapshot: Snapshot) {
        let taken = Arc::new(Taken {
            snapshot,
            at: Instant::now(),
        });
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(taken);
    }

    pub(crate) fn get(&self) -> Option<Arc<Taken>> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Taken {
    fn age(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.at)
    }

    /// Sampling is current while the newest sample is at most three
    /// intervals old, and 5 s at least: when the dashboard would say
    /// NO SAMPLE, a scrape says `mlxtop_up 0`.
    fn current(&self, now: Instant) -> bool {
        let limit = Duration::from_secs(self.snapshot.interval_s.saturating_mul(3));
        self.age(now) <= limit.max(Duration::from_secs(5))
    }
}

fn loopback(ip: IpAddr) -> bool {
    ip.to_canonical().is_loopback()
}

/// The address `--listen` names: a loopback address and a port, as
/// `127.0.0.1:10063`, `[::1]:10063` or `localhost:10063`. Any other address
/// is refused; another machine reaches the port through `ssh -L`.
pub(crate) fn listen_address(value: &str) -> Result<SocketAddr, String> {
    let address = match value.strip_prefix("localhost:") {
        Some(port) => port
            .parse()
            .ok()
            .map(|port: u16| SocketAddr::from((Ipv4Addr::LOCALHOST, port))),
        None => value.parse().ok(),
    }
    .ok_or_else(|| {
        format!(
            "--listen takes a loopback address and a port, as {DEFAULT_LISTEN} (got \"{value}\")"
        )
    })?;
    if !loopback(address.ip()) {
        let port = address.port();
        return Err(format!(
            "serve listens on loopback only (127.0.0.1 or [::1]), not {}: from another machine, forward the port with ssh -L {port}:127.0.0.1:{port} HOST",
            address.ip()
        ));
    }
    Ok(address)
}

/// A listener on `address`, which must be a loopback address.
pub(crate) fn bind(address: SocketAddr) -> io::Result<TcpListener> {
    if !loopback(address.ip()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "serve listens on loopback only",
        ));
    }
    TcpListener::bind(address)
}

/// `mlxtop serve`: sample with `build`'s collector every `interval` and
/// answer on `listener` until `stop` says so. Sampling that ends by itself
/// (a panic, which the log records) ends serving with an error.
pub(crate) fn serve(
    listener: TcpListener,
    interval: Duration,
    build: impl FnOnce() -> Collector + Send + 'static,
    stop: &dyn Fn() -> bool,
) -> io::Result<()> {
    let address = listener
        .local_addr()
        .map_or_else(|_| "unknown".to_owned(), |address| address.to_string());
    diagnostics_log(
        "INFO",
        "serve_start",
        format!("listen={address} interval_seconds={}", interval.as_secs()),
    );
    let latest = Latest::default();
    let stopping = AtomicBool::new(false);
    let ended = thread::scope(|scope| {
        let (slot, stopped) = (&latest, &stopping);
        let listening = scope.spawn(move || answer_all(listener, slot, stopped));
        let sampler = Sampler::headless(interval, build, latest.clone());
        let ended = loop {
            if stop() {
                break Ok(());
            }
            if sampler
                .handle
                .as_ref()
                .is_none_or(thread::JoinHandle::is_finished)
            {
                break Err(io::Error::other("sampling stopped; the log says why"));
            }
            if listening.is_finished() {
                // The join below says why.
                break Ok(());
            }
            thread::sleep(POLL);
        };
        stopping.store(true, Ordering::SeqCst);
        drop(sampler);
        let listened = listening
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the listener stopped")));
        listened.and(ended)
    });
    diagnostics_log(
        "INFO",
        "serve_stop",
        match &ended {
            Ok(()) => "reason=stopped".to_owned(),
            Err(error) => format!("error={}", log_field(&error.to_string())),
        },
    );
    ended
}

/// Answer connections on `listener` from `latest`, one at a time, until
/// `stop` is set.
pub(crate) fn answer_all(
    listener: TcpListener,
    latest: &Latest,
    stop: &AtomicBool,
) -> io::Result<()> {
    // Accept without blocking, so a stop is seen within one poll.
    listener.set_nonblocking(true)?;
    let mut failing = false;
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                failing = false;
                answer(stream, latest);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                thread::sleep(POLL);
            }
            Err(error) => {
                // Out of file descriptors and the like: logged once, retried.
                if !failing {
                    diagnostics_log(
                        "WARN",
                        "serve_accept_failed",
                        format!("error={}", log_field(&error.to_string())),
                    );
                    failing = true;
                }
                thread::sleep(POLL);
            }
        }
    }
    Ok(())
}

/// Answer one connection: read its head, at most 8 KiB within 2 seconds,
/// reply, and close.
fn answer(mut stream: TcpStream, latest: &Latest) {
    // An accepted socket keeps the listener's non-blocking mode on macOS.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(TIMEOUT));
    let reply = match read_head(&mut stream, Instant::now() + TIMEOUT) {
        Head::Complete(head) => respond(&head, latest.get().as_deref(), Instant::now()),
        Head::TooLarge => Reply::text(Status::TooLarge, "a request head is 8 KiB at most\n"),
        Head::Late => Reply::text(Status::Timeout, "a request head must arrive within 2 s\n"),
        Head::Closed => return,
    };
    let _ = stream.write_all(&reply.bytes());
}

/// How reading a request head ended.
enum Head {
    /// The request line and headers, without the blank line that ends them.
    Complete(Vec<u8>),
    TooLarge,
    /// The deadline passed first: a client that stalls or trickles.
    Late,
    Closed,
}

/// Read a request head until its blank line, `HEAD_LIMIT` bytes or
/// `deadline`, whichever comes first. The deadline holds for the whole
/// head, so a client cannot hold the listener by sending a byte at a time.
fn read_head(stream: &mut TcpStream, deadline: Instant) -> Head {
    let mut head = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            return Head::Late;
        }
        let room = chunk.len().min(HEAD_LIMIT - head.len());
        let read = match stream.read(&mut chunk[..room]) {
            Ok(0) => return Head::Closed,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Head::Late;
            }
            Err(_) => return Head::Closed,
        };
        // The blank line may straddle the last read and this one.
        let from = head.len().saturating_sub(3);
        head.extend_from_slice(&chunk[..read]);
        if let Some(end) = head[from..].windows(4).position(|four| four == b"\r\n\r\n") {
            head.truncate(from + end);
            return Head::Complete(head);
        }
        if head.len() >= HEAD_LIMIT {
            return Head::TooLarge;
        }
    }
}

/// The parts of a request head an answer depends on.
struct Request<'a> {
    method: &'a str,
    /// The target without its query.
    path: &'a str,
    /// The Host header, when there is exactly one.
    host: Option<&'a str>,
}

impl<'a> Request<'a> {
    /// `None` for a head that is not an HTTP/1 request.
    fn parse(head: &'a str) -> Option<Self> {
        let mut lines = head.split("\r\n");
        let mut words = lines.next()?.split(' ');
        let (method, target, version) = (words.next()?, words.next()?, words.next()?);
        if words.next().is_some()
            || method.is_empty()
            || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        {
            return None;
        }
        let mut hosts = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':')?;
            if name.is_empty() || name.contains([' ', '\t']) {
                return None;
            }
            if name.eq_ignore_ascii_case("host") {
                hosts.push(value.trim_matches([' ', '\t']));
            }
        }
        Some(Self {
            method,
            path: target.split_once('?').map_or(target, |(path, _)| path),
            host: match hosts[..] {
                [host] => Some(host),
                _ => None,
            },
        })
    }
}

/// Whether a Host header names this machine's loopback: `localhost`, an
/// address in 127.0.0.0/8 or `[::1]`, with any port or none (a tunnel's
/// local port need not be the one served). Any other name means a request
/// meant for another host, or one from a web page whose name a DNS server
/// pointed here.
fn loopback_host(host: &str) -> bool {
    let (name, port) = if let Some(rest) = host.strip_prefix('[') {
        let Some((address, after)) = rest.split_once(']') else {
            return false;
        };
        match (after, after.strip_prefix(':')) {
            ("", _) => (address, None),
            (_, Some(port)) => (address, Some(port)),
            (_, None) => return false,
        }
    } else {
        match host.split_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (host, None),
        }
    };
    if port.is_some_and(|port| port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit())) {
        return false;
    }
    name.eq_ignore_ascii_case("localhost") || name.parse().is_ok_and(loopback)
}

/// An answer's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    Timeout,
    TooLarge,
    Unavailable,
}

impl Status {
    fn line(self) -> &'static str {
        match self {
            Self::Ok => "200 OK",
            Self::BadRequest => "400 Bad Request",
            Self::Forbidden => "403 Forbidden",
            Self::NotFound => "404 Not Found",
            Self::MethodNotAllowed => "405 Method Not Allowed",
            Self::Timeout => "408 Request Timeout",
            Self::TooLarge => "431 Request Header Fields Too Large",
            Self::Unavailable => "503 Service Unavailable",
        }
    }
}

/// An answer, before it is written.
pub(crate) struct Reply {
    pub(crate) status: Status,
    pub(crate) content_type: &'static str,
    pub(crate) body: Vec<u8>,
    /// Answering HEAD: a GET's headers, without its body.
    pub(crate) head_only: bool,
}

impl Reply {
    fn new(status: Status, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            body,
            head_only: false,
        }
    }

    fn text(status: Status, body: impl Into<String>) -> Self {
        Self::new(status, TEXT_TYPE, body.into().into_bytes())
    }

    /// The answer as sent: one connection, one answer, then closed.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        let allow = if self.status == Status::MethodNotAllowed {
            "Allow: GET, HEAD\r\n"
        } else {
            ""
        };
        let mut bytes = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\n{allow}X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
            self.status.line(),
            self.content_type,
            self.body.len()
        )
        .into_bytes();
        if !self.head_only {
            bytes.extend_from_slice(&self.body);
        }
        bytes
    }
}

/// The answer to the request `head` (its request line and headers, without
/// the blank line) from the newest sample, `latest`, at `now`.
pub(crate) fn respond(head: &[u8], latest: Option<&Taken>, now: Instant) -> Reply {
    let Some(request) = std::str::from_utf8(head).ok().and_then(Request::parse) else {
        return Reply::text(Status::BadRequest, "not an HTTP/1 request\n");
    };
    let mut reply = route(&request, latest, now);
    reply.head_only = request.method == "HEAD";
    reply
}

fn route(request: &Request, latest: Option<&Taken>, now: Instant) -> Reply {
    match request.host {
        None => return Reply::text(Status::BadRequest, "a request names one Host\n"),
        Some(host) if !loopback_host(host) => {
            return Reply::text(
                Status::Forbidden,
                "mlxtop answers only requests addressed to localhost, 127.0.0.1 or [::1]\n",
            )
        }
        Some(_) => {}
    }
    if !matches!(request.method, "GET" | "HEAD") {
        return Reply::text(Status::MethodNotAllowed, "only GET and HEAD\n");
    }
    match request.path {
        "/metrics" => Reply::new(Status::Ok, METRICS_TYPE, metrics(latest, now).into_bytes()),
        "/snapshot" => {
            let mut body = Vec::new();
            match latest.map(|taken| crate::export::write_line(&mut body, &taken.snapshot)) {
                Some(Ok(())) => Reply::new(Status::Ok, JSON_TYPE, body),
                _ => Reply::text(Status::Unavailable, "no sample yet\n"),
            }
        }
        "/health" => match latest {
            Some(taken) if taken.current(now) => Reply::text(Status::Ok, "ok\n"),
            Some(taken) => Reply::text(
                Status::Unavailable,
                format!("no sample for {}s\n", taken.age(now).as_secs()),
            ),
            None => Reply::text(Status::Unavailable, "no sample yet\n"),
        },
        _ => Reply::text(
            Status::NotFound,
            "mlxtop serves /metrics, /snapshot and /health\n",
        ),
    }
}

// --- the Prometheus text -----------------------------------------------------

const GAUGE: &str = "gauge";
const COUNTER: &str = "counter";

/// One reading of the snapshot: its name, type, meaning and value, `None`
/// when the reading is unknown.
pub(crate) struct Metric<T> {
    pub(crate) name: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) help: &'static str,
    pub(crate) value: fn(&T) -> Option<f64>,
}

const fn gauge<T>(
    name: &'static str,
    help: &'static str,
    value: fn(&T) -> Option<f64>,
) -> Metric<T> {
    Metric {
        name,
        kind: GAUGE,
        help,
        value,
    }
}

const fn counter<T>(
    name: &'static str,
    help: &'static str,
    value: fn(&T) -> Option<f64>,
) -> Metric<T> {
    Metric {
        name,
        kind: COUNTER,
        help,
        value,
    }
}

fn whole(value: Option<u64>) -> Option<f64> {
    value.map(|value| value as f64)
}

fn signed(value: Option<i64>) -> Option<f64> {
    value.map(|value| value as f64)
}

fn flag(value: Option<bool>) -> Option<f64> {
    value.map(|value| f64::from(u8::from(value)))
}

/// A named level's place among `levels`, lowest first.
fn level(value: Option<&str>, levels: &[&str]) -> Option<f64> {
    let value = value?;
    levels
        .iter()
        .position(|level| *level == value)
        .map(|place| place as f64)
}

/// A speed, when it is of the `wanted` kind: `live`, `average` or
/// `last_request`.
fn rate(tok_per_s: Option<f64>, basis: Option<&str>, wanted: &str) -> Option<f64> {
    tok_per_s.filter(|_| basis == Some(wanted))
}

/// The usual speed's `figure`, while it is the usual for `phase`.
fn usual(
    snapshot: &Snapshot,
    phase: &str,
    figure: fn(&crate::export::UsualSpeed) -> Option<f64>,
) -> Option<f64> {
    snapshot
        .usual_speed
        .as_ref()
        .filter(|usual| usual.phase == phase)
        .and_then(figure)
}

/// Every reading with no label, in the order written.
pub(crate) const READINGS: &[Metric<Snapshot>] = &[
    gauge(
        "mlxtop_interval_seconds",
        "Seconds between samples: the span each rate is measured over",
        |s| Some(s.interval_s as f64),
    ),
    gauge("mlxtop_ram_total_bytes", "All of RAM", |s| {
        whole(s.host.ram_bytes)
    }),
    gauge(
        "mlxtop_ram_available_ratio",
        "The share of RAM the operating system reports available",
        |s| s.host.memory.available_ratio,
    ),
    gauge(
        "mlxtop_memory_pressure_level",
        "The operating system's memory pressure: 0 normal, 1 warning, 2 critical",
        |s| level(s.host.memory.pressure, &["normal", "warning", "critical"]),
    ),
    gauge(
        "mlxtop_memory_tight",
        "1 while pressure is raised or little memory is free, so file paging counts against the machine",
        |s| flag(s.host.memory.tight),
    ),
    gauge(
        "mlxtop_ram_used_bytes",
        "Wired, app and compressed memory: what the RAM meter fills",
        |s| whole(s.host.memory.ram.used_bytes),
    ),
    gauge(
        "mlxtop_ram_left_bytes",
        "Free memory and file cache: what a model can still grow into",
        |s| whole(s.host.memory.ram.left_bytes),
    ),
    gauge(
        "mlxtop_ram_wired_bytes",
        "Memory that cannot be paged out",
        |s| whole(s.host.memory.ram.wired_bytes),
    ),
    gauge("mlxtop_ram_app_bytes", "Apps' memory", |s| {
        whole(s.host.memory.ram.app_bytes)
    }),
    gauge(
        "mlxtop_ram_compressed_bytes",
        "What the memory compressor holds (macOS)",
        |s| whole(s.host.memory.ram.compressed_bytes),
    ),
    gauge("mlxtop_ram_file_cache_bytes", "File cache", |s| {
        whole(s.host.memory.ram.file_cache_bytes)
    }),
    gauge("mlxtop_ram_free_bytes", "Free memory", |s| {
        whole(s.host.memory.ram.free_bytes)
    }),
    gauge(
        "mlxtop_gpu_memory_used_bytes",
        "What the GPU holds of RAM, where it has no memory of its own",
        |s| whole(s.host.memory.gpu.as_ref()?.used_bytes),
    ),
    gauge(
        "mlxtop_gpu_memory_limit_bytes",
        "The GPU's wired-memory limit, where it shares RAM",
        |s| whole(s.host.memory.gpu.as_ref()?.limit_bytes),
    ),
    gauge(
        "mlxtop_gpu_memory_left_bytes",
        "The room left under that limit; 0 once over it",
        |s| whole(s.host.memory.gpu.as_ref()?.left_bytes),
    ),
    gauge(
        "mlxtop_vram_used_bytes",
        "The cards' own memory in use, all cards together",
        |s| whole(s.host.memory.vram.as_ref().map(|vram| vram.used_bytes)),
    ),
    gauge(
        "mlxtop_vram_total_bytes",
        "The cards' own memory, all cards together",
        |s| whole(s.host.memory.vram.as_ref().map(|vram| vram.total_bytes)),
    ),
    gauge(
        "mlxtop_vram_left_bytes",
        "The cards' own memory left, all cards together",
        |s| whole(s.host.memory.vram.as_ref().map(|vram| vram.left_bytes)),
    ),
    gauge("mlxtop_swap_total_bytes", "Swap allocated", |s| {
        whole(s.host.memory.swap.total_bytes)
    }),
    gauge("mlxtop_swap_used_bytes", "Swap in use", |s| {
        whole(s.host.memory.swap.used_bytes)
    }),
    gauge(
        "mlxtop_swap_in_bytes_per_second",
        "Swapped in, over the last interval",
        |s| whole(s.host.memory.swap.in_bytes_per_s),
    ),
    gauge(
        "mlxtop_swap_out_bytes_per_second",
        "Swapped out, over the last interval",
        |s| whole(s.host.memory.swap.out_bytes_per_s),
    ),
    gauge(
        "mlxtop_swap_growth_bytes_per_second",
        "How fast swap use grows; negative while it shrinks",
        |s| signed(s.host.memory.swap.growth_bytes_per_s),
    ),
    gauge(
        "mlxtop_file_paging_in_bytes_per_second",
        "Pages read from disk outside swap (macOS): model loads, and a full Mac's paging with swap off",
        |s| whole(s.host.memory.file_paging.in_bytes_per_s),
    ),
    gauge(
        "mlxtop_file_paging_out_bytes_per_second",
        "Pages written to disk outside swap (macOS)",
        |s| whole(s.host.memory.file_paging.out_bytes_per_s),
    ),
    gauge(
        "mlxtop_memory_compressed_bytes_per_second",
        "Memory compressed, over the last interval (macOS)",
        |s| whole(s.host.memory.compression.compress_bytes_per_s),
    ),
    gauge(
        "mlxtop_memory_decompressed_bytes_per_second",
        "Memory decompressed, over the last interval (macOS)",
        |s| whole(s.host.memory.compression.decompress_bytes_per_s),
    ),
    gauge(
        "mlxtop_published_bandwidth_bytes_per_second",
        "Estimate: the peak memory bandwidth the maker publishes for this chip (the Mac's, or the one card)",
        |s| whole(s.host.published_bandwidth_bytes_per_s),
    ),
    gauge(
        "mlxtop_thermal_pressure_level",
        "macOS's thermal pressure: 0 nominal, 1 moderate, 2 heavy, 3 trapping, 4 sleeping",
        |s| {
            level(
                s.host.thermal.level,
                &["nominal", "moderate", "heavy", "trapping", "sleeping"],
            )
        },
    ),
    gauge(
        "mlxtop_thermal_limited",
        "1 while macOS holds the machine back for heat",
        |s| flag(s.host.thermal.limited),
    ),
    gauge(
        "mlxtop_cpu_speed_limit_ratio",
        "The share of full CPU speed macOS allows (Intel Macs)",
        |s| s.host.thermal.cpu_speed_limit_ratio,
    ),
    gauge(
        "mlxtop_hottest_temperature_celsius",
        "The hottest temperature measured: thermal zones and GPUs on Linux, GPUs on Windows",
        |s| whole(s.host.thermal.hottest_celsius),
    ),
    gauge(
        "mlxtop_system_power_watts",
        "What the whole machine draws (macOS)",
        |s| s.host.system_power_w,
    ),
    gauge("mlxtop_runtime_processes", "The runtime's processes", |s| {
        Some(f64::from(s.host.runtime_process.count))
    }),
    gauge(
        "mlxtop_runtime_process_memory_bytes",
        "What the runtime holds: its largest process's footprint or all its processes' resident size, whichever is more",
        |s| whole(s.host.runtime_process.memory_bytes),
    ),
    gauge(
        "mlxtop_runtime_process_peak_bytes",
        "Its largest process's lifetime peak footprint (macOS)",
        |s| whole(s.host.runtime_process.peak_bytes),
    ),
    gauge(
        "mlxtop_runtime_process_growth_bytes_per_second",
        "How fast that footprint grows",
        |s| signed(s.host.runtime_process.growth_bytes_per_s),
    ),
    gauge(
        "mlxtop_runtime_process_cpu_cores",
        "CPU the runtime's processes use, in cores",
        |s| s.host.runtime_process.cpu_cores,
    ),
    gauge(
        "mlxtop_runtime_process_disk_read_bytes_per_second",
        "The runtime's storage reads (macOS)",
        |s| whole(s.host.runtime_process.disk_read_bytes_per_s),
    ),
    gauge(
        "mlxtop_runtime_process_disk_write_bytes_per_second",
        "The runtime's storage writes (macOS)",
        |s| whole(s.host.runtime_process.disk_write_bytes_per_s),
    ),
    gauge(
        "mlxtop_llm_up",
        "1 while the runtime's API answers, 0 when it does not",
        |s| flag(s.runtime.connected),
    ),
    gauge(
        "mlxtop_llm_remote",
        "1 when the runtime runs on another machine: the host readings are not its own",
        |s| flag(s.runtime.provider.as_ref().map(|_| s.runtime.remote)),
    ),
    gauge(
        "mlxtop_llm_telemetry_age_seconds",
        "How old the runtime's readings were when the sample was taken",
        |s| {
            let age = s.ts_ms?.checked_sub(s.runtime.observed_at_ms?)?;
            Some(age as f64 / 1000.0)
        },
    ),
    gauge(
        "mlxtop_llm_generation_tokens_per_second",
        "Generation speed over the last interval",
        |s| {
            let generation = &s.runtime.generation;
            rate(generation.tok_per_s, generation.basis, "live")
        },
    ),
    gauge(
        "mlxtop_llm_generation_average_tokens_per_second",
        "The runtime's own average generation speed, while it reports no live one",
        |s| {
            let generation = &s.runtime.generation;
            rate(generation.tok_per_s, generation.basis, "average")
        },
    ),
    gauge(
        "mlxtop_llm_last_request_generation_tokens_per_second",
        "The last finished request's generation speed, from a completion log or a usage file",
        |s| {
            let generation = &s.runtime.generation;
            rate(generation.tok_per_s, generation.basis, "last_request")
        },
    ),
    gauge(
        "mlxtop_llm_prefill_tokens_per_second",
        "Prompt reading speed over the last interval",
        |s| rate(s.runtime.prefill.tok_per_s, s.runtime.prefill.basis, "live"),
    ),
    gauge(
        "mlxtop_llm_prefill_average_tokens_per_second",
        "The runtime's own average prompt reading speed, while it reports no live one",
        |s| rate(s.runtime.prefill.tok_per_s, s.runtime.prefill.basis, "average"),
    ),
    gauge(
        "mlxtop_llm_last_request_prefill_tokens_per_second",
        "The last finished request's prompt reading speed",
        |s| {
            let prefill = &s.runtime.prefill;
            rate(prefill.tok_per_s, prefill.basis, "last_request")
        },
    ),
    gauge(
        "mlxtop_llm_prefill_done_tokens",
        "Prompt tokens the prefill in flight has read",
        |s| whole(s.runtime.prefill.done_tokens),
    ),
    gauge(
        "mlxtop_llm_prefill_expected_tokens",
        "Prompt tokens the prefill in flight will read",
        |s| whole(s.runtime.prefill.expected_tokens),
    ),
    gauge("mlxtop_llm_requests_active", "Requests running", |s| {
        whole(s.runtime.requests.active)
    }),
    gauge("mlxtop_llm_requests_waiting", "Requests queued", |s| {
        whole(s.runtime.requests.waiting)
    }),
    gauge(
        "mlxtop_llm_requests_prefilling",
        "Running requests reading their prompt, where the runtime lists its requests",
        |s| whole(s.runtime.requests.prefilling),
    ),
    gauge(
        "mlxtop_llm_requests_generating",
        "Running requests writing their answer, where the runtime lists its requests",
        |s| whole(s.runtime.requests.generating),
    ),
    gauge(
        "mlxtop_llm_context_tokens",
        "Tokens the requests writing an answer hold, prompt and output together",
        |s| whole(s.runtime.requests.context_tokens),
    ),
    gauge(
        "mlxtop_llm_model_memory_bytes",
        "Memory the runtime's models take",
        |s| whole(s.runtime.model_memory_bytes),
    ),
    gauge(
        "mlxtop_llm_model_memory_ceiling_bytes",
        "The most memory the runtime lets its models take",
        |s| whole(s.runtime.model_memory_ceiling_bytes),
    ),
    gauge(
        "mlxtop_llm_model_size_bytes",
        "The loaded model's size (Ollama)",
        |s| whole(s.runtime.model_size_bytes),
    ),
    gauge(
        "mlxtop_llm_model_offloaded_bytes",
        "The part of the model placed outside GPU memory (Ollama)",
        |s| whole(s.runtime.model_offloaded_bytes),
    ),
    gauge(
        "mlxtop_llm_context_allowed_tokens",
        "The context the runtime allows for the model",
        |s| whole(s.runtime.context_allowed_tokens),
    ),
    gauge(
        "mlxtop_llm_weight_bytes_per_pass",
        "Estimate: weight bytes one forward pass reads for a token",
        |s| whole(s.runtime.model_facts.as_ref()?.weight_bytes_per_pass),
    ),
    gauge(
        "mlxtop_llm_bits_per_weight",
        "Estimate: bits per parameter a pass reads, scales and biases included",
        |s| s.runtime.model_facts.as_ref()?.bits_per_weight,
    ),
    gauge(
        "mlxtop_llm_kv_bytes_per_token",
        "KV-cache bytes one more token of context costs: the runtime's figure where it reports one, else an estimate from the model's layers",
        |s| whole(s.runtime.model_facts.as_ref()?.kv_bytes_per_token),
    ),
    gauge(
        "mlxtop_llm_state_bytes_per_sequence",
        "Estimate: what a sequence holds whatever its length (linear-attention state, sliding windows, a draft's window)",
        |s| whole(s.runtime.model_facts.as_ref()?.state_bytes_per_sequence),
    ),
    gauge(
        "mlxtop_llm_context_trained_tokens",
        "Estimate: the context the model was trained for, as its config.json declares it",
        |s| whole(s.runtime.model_facts.as_ref()?.context_trained_tokens),
    ),
    gauge(
        "mlxtop_llm_draft_weight_bytes",
        "Estimate: the draft model's weight bytes as stored, where speculative decoding drafts",
        |s| whole(s.runtime.model_facts.as_ref()?.draft_weight_bytes),
    ),
    gauge(
        "mlxtop_llm_mlx_active_bytes",
        "MLX's active memory in the runtime",
        |s| whole(s.runtime.mlx.as_ref()?.active_bytes),
    ),
    gauge("mlxtop_llm_mlx_cache_bytes", "MLX's cache", |s| {
        whole(s.runtime.mlx.as_ref()?.cache_bytes)
    }),
    gauge("mlxtop_llm_mlx_peak_bytes", "MLX's peak memory", |s| {
        whole(s.runtime.mlx.as_ref()?.peak_bytes)
    }),
    gauge(
        "mlxtop_llm_mlx_process_footprint_bytes",
        "The serving process's footprint, as the runtime reports it",
        |s| whole(s.runtime.mlx.as_ref()?.process_footprint_bytes),
    ),
    gauge(
        "mlxtop_llm_cache_reuse_ratio",
        "The share of prompt tokens served from cache since the runtime started",
        |s| s.runtime.cache.reuse_ratio,
    ),
    gauge(
        "mlxtop_llm_cache_recent_reuse_ratio",
        "The share of prompt tokens served from cache over the last interval",
        |s| s.runtime.cache.recent_reuse_ratio,
    ),
    gauge(
        "mlxtop_llm_prefix_cache_hit_ratio",
        "Prefix-cache hits",
        |s| s.runtime.cache.prefix_hit_ratio,
    ),
    counter(
        "mlxtop_llm_prompt_tokens_total",
        "Prompt tokens the runtime was sent, by its own count since it started",
        |s| whole(s.runtime.cache.prompt_tokens_total),
    ),
    counter(
        "mlxtop_llm_cached_prompt_tokens_total",
        "Of those, the ones served from its cache",
        |s| whole(s.runtime.cache.cached_tokens_total),
    ),
    gauge(
        "mlxtop_llm_ssd_cache_used_bytes",
        "The runtime's prompt cache on disk (oMLX)",
        |s| whole(s.runtime.cache.ssd_used_bytes),
    ),
    gauge(
        "mlxtop_llm_ssd_cache_capacity_bytes",
        "The size of that cache",
        |s| whole(s.runtime.cache.ssd_capacity_bytes),
    ),
    gauge(
        "mlxtop_llm_draft_acceptance_ratio",
        "The share of drafted tokens accepted since the last poll (speculative decoding)",
        |s| s.runtime.draft.acceptance_ratio,
    ),
    gauge(
        "mlxtop_llm_usual_generation_tokens_per_second",
        "Estimate: the speed earlier samples of similar generation ran at",
        |s| usual(s, "generation", |usual| usual.tok_per_s),
    ),
    gauge(
        "mlxtop_llm_generation_slow_below_tokens_per_second",
        "Estimate: below this speed, this generation is slow",
        |s| usual(s, "generation", |usual| usual.slow_below_tok_per_s),
    ),
    gauge(
        "mlxtop_llm_usual_prefill_tokens_per_second",
        "Estimate: the speed earlier samples of similar prompt reading ran at",
        |s| usual(s, "prefill", |usual| usual.tok_per_s),
    ),
    gauge(
        "mlxtop_llm_prefill_slow_below_tokens_per_second",
        "Estimate: below this speed, this prompt reading is slow",
        |s| usual(s, "prefill", |usual| usual.slow_below_tok_per_s),
    ),
    gauge(
        "mlxtop_llm_slowdown_seconds",
        "Estimate: how long the slowdown under way has lasted",
        |s| s.slowdown.as_ref()?.duration_s,
    ),
    gauge(
        "mlxtop_llm_stall_seconds",
        "Estimate: how long a stalled answer has gone without a token",
        |s| s.slowdown.as_ref()?.stalled_s,
    ),
];

/// Every reading of one GPU, labelled with its index.
pub(crate) const GPU_READINGS: &[Metric<Gpu>] = &[
    gauge("mlxtop_gpu_busy_ratio", "How busy the GPU is", |g| {
        g.busy_ratio
    }),
    gauge(
        "mlxtop_gpu_renderer_busy_ratio",
        "How busy the Apple GPU's renderer is",
        |g| g.renderer_ratio,
    ),
    gauge(
        "mlxtop_gpu_tiler_busy_ratio",
        "How busy the Apple GPU's tiler is",
        |g| g.tiler_ratio,
    ),
    gauge(
        "mlxtop_gpu_vram_used_bytes",
        "A card's own memory in use",
        |g| whole(g.memory_used_bytes),
    ),
    gauge("mlxtop_gpu_vram_total_bytes", "A card's own memory", |g| {
        whole(g.memory_total_bytes)
    }),
    gauge(
        "mlxtop_gpu_temperature_celsius",
        "A card's temperature",
        |g| whole(g.temperature_celsius),
    ),
    gauge(
        "mlxtop_gpu_throttled",
        "1 while the driver holds a card's clocks down: heat, power brake, hardware slowdown or power cap",
        |g| g.throttle.map(|reason| f64::from(u8::from(reason != "none"))),
    ),
];

/// Every verdict, for the state set. Each names the next, so a verdict
/// added to diagnosis.rs does not compile until it has a place here.
pub(crate) fn verdicts() -> impl Iterator<Item = Verdict> {
    use Verdict::*;
    std::iter::successors(Some(Healthy), |verdict| match verdict {
        Healthy => Some(NoBottleneck),
        NoBottleneck => Some(CollectingBaseline),
        CollectingBaseline => Some(CountersIncomplete),
        CountersIncomplete => Some(GenerationSlowed),
        GenerationSlowed => Some(PrefillSlowed),
        PrefillSlowed => Some(GenerationStalled),
        GenerationStalled => Some(RequestsWaiting),
        RequestsWaiting => Some(MemoryBottleneck),
        MemoryBottleneck => Some(MemoryStress),
        MemoryStress => Some(SwapThrashing),
        SwapThrashing => Some(HeavyPaging),
        HeavyPaging => Some(PageInRecovery),
        PageInRecovery => Some(PagingActive),
        PagingActive => Some(WatchPaging),
        WatchPaging => Some(CompressionActive),
        CompressionActive => Some(ThermalLimit),
        ThermalLimit => Some(CpuOffload),
        CpuOffload => Some(VramFull),
        VramFull => Some(GpuThrottled),
        GpuThrottled => None,
    })
}

/// A verdict by the name `verdict.state` gives it in the JSON.
fn state_name(verdict: Verdict) -> String {
    serde_json::to_value(verdict)
        .ok()
        .and_then(|name| name.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// A label's value, escaped as the text format asks.
fn label(value: &str) -> String {
    value
        .replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
}

/// The text exposition format, one metric family at a time.
#[derive(Default)]
struct Text(String);

impl Text {
    /// A family's HELP and TYPE, then each sample (its labels already
    /// written); nothing at all when no value is known.
    fn family(&mut self, name: &str, kind: &str, help: &str, samples: &[(String, f64)]) {
        if !samples.iter().any(|(_, value)| value.is_finite()) {
            return;
        }
        let help = help.replace('\\', r"\\").replace('\n', r"\n");
        let _ = writeln!(self.0, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (labels, value) in samples.iter().filter(|(_, value)| value.is_finite()) {
            let _ = writeln!(self.0, "{name}{labels} {value}");
        }
    }

    fn one(&mut self, name: &str, kind: &str, help: &str, value: Option<f64>) {
        let samples: Vec<(String, f64)> = value
            .map(|value| (String::new(), value))
            .into_iter()
            .collect();
        self.family(name, kind, help, &samples);
    }
}

/// The Prometheus text for the newest sample, `latest`, at `now`. While
/// sampling is not current, `mlxtop_up` is 0 and no reading is written:
/// an old reading would be stored as the present one.
pub(crate) fn metrics(latest: Option<&Taken>, now: Instant) -> String {
    let mut text = Text::default();
    let current = latest.filter(|taken| taken.current(now));
    text.one(
        "mlxtop_up",
        GAUGE,
        "1 while sampling is current (the newest sample at most three intervals old, 5 s at least); 0 otherwise, and then no reading is written",
        Some(f64::from(u8::from(current.is_some()))),
    );
    if let Some(taken) = latest {
        let age = taken.age(now).as_millis() as f64 / 1000.0;
        text.one(
            "mlxtop_sample_age_seconds",
            GAUGE,
            "Seconds since the newest sample was taken",
            Some(age),
        );
    }
    if let Some(taken) = current {
        readings(&mut text, &taken.snapshot);
    }
    text.0
}

/// Everything one current snapshot says.
fn readings(text: &mut Text, snapshot: &Snapshot) {
    let states: Vec<(String, f64)> = verdicts()
        .map(|verdict| {
            let now = f64::from(u8::from(verdict == snapshot.verdict.state));
            (format!("{{limiter=\"{}\"}}", state_name(verdict)), now)
        })
        .collect();
    text.family(
        "mlxtop_bottleneck",
        GAUGE,
        "The verdict as a state set: 1 for its state (verdict.state in the JSON snapshot), 0 for every other",
        &states,
    );
    if let Some(provider) = &snapshot.runtime.provider {
        let labels = format!(
            "{{provider=\"{}\",model=\"{}\"}}",
            label(provider),
            label(snapshot.runtime.model.as_deref().unwrap_or_default())
        );
        text.family(
            "mlxtop_llm_model_info",
            GAUGE,
            "The runtime found and the model it serves",
            &[(labels, 1.0)],
        );
    }
    for metric in READINGS {
        text.one(
            metric.name,
            metric.kind,
            metric.help,
            (metric.value)(snapshot),
        );
    }
    for metric in GPU_READINGS {
        let samples: Vec<(String, f64)> = snapshot
            .host
            .gpus
            .iter()
            .filter_map(|gpu| {
                (metric.value)(gpu).map(|value| (format!("{{gpu=\"{}\"}}", gpu.index), value))
            })
            .collect();
        text.family(metric.name, metric.kind, metric.help, &samples);
    }
}

#[cfg(test)]
#[path = "tests/exporter.rs"]
mod tests;
