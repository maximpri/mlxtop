// SPDX-License-Identifier: MIT
//! Bounded oMLX HTTP transport; authentication policy belongs to the provider.
use crate::runtime_diagnostics::ProbeIssue;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
pub(crate) const MAX_HTTP_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl HttpResponse {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The longest one oMLX request may take, reply included.
const REPLY_BUDGET: Duration = Duration::from_secs(2);

/// Whether `raw` holds a whole reply by its `Content-Length` header.
fn content_complete(raw: &[u8]) -> bool {
    let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..end]);
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .is_some_and(|length| raw.len() >= end + 4 + length)
}

pub(crate) fn http_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<HttpResponse, ProbeIssue> {
    // One deadline for the whole exchange (and within the poll's own).
    let budget = crate::providers::request_budget(REPLY_BUDGET).ok_or(ProbeIssue::Timeout)?;
    let deadline = Instant::now() + budget;
    // `localhost` resolves to ::1 first on many machines while the server
    // listens on IPv4 only: try each address.
    let addresses: Vec<_> = (host.trim_matches(['[', ']']), port)
        .to_socket_addrs()
        .map_err(ProbeIssue::from_io)?
        .collect();
    let mut connected = Err(ProbeIssue::Transport);
    for address in &addresses {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ProbeIssue::Timeout);
        }
        connected = TcpStream::connect_timeout(address, left.min(Duration::from_millis(250)))
            .map_err(|error| ProbeIssue::from_connect(error, address));
        if connected.is_ok() {
            break;
        }
    }
    let mut stream = connected?;
    stream
        .set_write_timeout(Some(Duration::from_millis(250)))
        .map_err(ProbeIssue::from_io)?;
    let body = body.unwrap_or("");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n");
    for (key, value) in headers {
        request.push_str(key);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    if !body.is_empty() {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream
        .write_all(request.as_bytes())
        .map_err(ProbeIssue::from_io)?;

    // Read until the server closes, the reply is complete by its
    // Content-Length, or the deadline passes: a slow drip cannot hold the
    // sampler, and a server that keeps the connection open is still read.
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        if content_complete(&raw) {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ProbeIssue::Timeout);
        }
        stream
            .set_read_timeout(Some(left))
            .map_err(ProbeIssue::from_io)?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if raw.len() > MAX_HTTP_RESPONSE_BYTES {
                    return Err(ProbeIssue::ResponseTooLarge);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ProbeIssue::from_io(error)),
        }
    }
    if raw.len() > MAX_HTTP_RESPONSE_BYTES {
        return Err(ProbeIssue::ResponseTooLarge);
    }
    let raw = String::from_utf8_lossy(&raw);
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or(ProbeIssue::InvalidResponse)?;
    let mut lines = head.lines();
    let status = lines
        .next()
        .ok_or(ProbeIssue::InvalidResponse)?
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .ok_or(ProbeIssue::InvalidResponse)?;
    let headers = lines
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    Ok(HttpResponse {
        status,
        headers,
        body: body.to_owned(),
    })
}
