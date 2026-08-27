//! A minimal loopback client for this node's own HTTP API.
//!
//! It exists for exactly one reason: some operations can only be performed by
//! the process that holds the transport sessions. A separate CLI process cannot
//! dial a peer this node is already connected to — `register` refuses a second
//! connection, and correctly so, because two sessions with one peer would give
//! the Health Plane two cursors for the same node. So the CLI asks the running
//! service to act instead of racing it.
//!
//! Deliberately not a dependency. This speaks the small, fixed subset of HTTP
//! the local API needs — one JSON POST to a loopback address with a bearer
//! token — rather than pulling a client library in for a single verb. There is
//! no TLS here and none is wanted: the target is the address this node bound
//! itself, on this machine.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Longest a local request may take to connect and send.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A dispatch may legitimately wait on a remote run, so reads get their own,
/// caller-supplied budget rather than the connect budget.
const READ_HEADROOM: Duration = Duration::from_secs(15);
/// Bounded so a misbehaving local service cannot make the CLI grow without end.
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub enum LocalApiError {
    /// Nothing is listening. The caller decides what to do instead; this is not
    /// a failure of the operation, only of this route to it.
    Unreachable,
    Io(std::io::Error),
    /// The service answered, but not with something this client understands.
    Malformed(String),
    /// The service answered with a refusal, carrying its own envelope.
    Refused(serde_json::Value),
}

impl std::fmt::Display for LocalApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable => write!(f, "no local node service is listening"),
            Self::Io(error) => write!(f, "local node service I/O failed: {error}"),
            Self::Malformed(detail) => write!(f, "local node service replied oddly: {detail}"),
            Self::Refused(envelope) => write!(f, "local node service refused: {envelope}"),
        }
    }
}

/// POST one JSON body to this node's own API and return the `data` object.
pub fn post_json(
    addr: SocketAddr,
    token: &str,
    path: &str,
    body: &serde_json::Value,
    read_timeout: Duration,
) -> Result<serde_json::Value, LocalApiError> {
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
        .map_err(|_| LocalApiError::Unreachable)?;
    stream
        .set_write_timeout(Some(CONNECT_TIMEOUT))
        .map_err(LocalApiError::Io)?;
    stream
        .set_read_timeout(Some(read_timeout + READ_HEADROOM))
        .map_err(LocalApiError::Io)?;

    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(LocalApiError::Io)?;
    stream.flush().map_err(LocalApiError::Io)?;

    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if raw.len() > MAX_RESPONSE_BYTES {
                    return Err(LocalApiError::Malformed("response too large".into()));
                }
            }
            Err(error) => return Err(LocalApiError::Io(error)),
        }
    }

    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| LocalApiError::Malformed("no header terminator".into()))?;
    let payload = &raw[split + 4..];
    let envelope: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|error| LocalApiError::Malformed(error.to_string()))?;
    if envelope.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(LocalApiError::Refused(envelope));
    }
    Ok(envelope
        .get("data")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}
