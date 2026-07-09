#![allow(dead_code)]

use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicU16, Ordering};
use std::thread;
use std::time::{Duration, Instant};

static NEXT_PORT: AtomicU16 = AtomicU16::new(39_000);

pub fn omakure_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omakure"))
}

pub fn omakure_command() -> Command {
    Command::new(omakure_bin())
}

pub struct TestWorkspace {
    dir: tempfile::TempDir,
}

impl TestWorkspace {
    pub fn new(label: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("omakure_{label}_"))
            .tempdir()
            .expect("create test workspace");
        Self { dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn write_schema_script(&self, name: &str, schema_name: &str, body: &str) -> PathBuf {
        let path = self.path().join(name);
        let script = format!(
            r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {{
#   "Name": "{}",
#   "Description": "test fixture",
#   "Fields": []
# }}
# OMAKURE_SCHEMA_END
{}
"#,
            schema_name, body
        );
        fs::write(&path, script).expect("write schema script fixture");
        path
    }
}

#[cfg(unix)]
pub fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .expect("read script metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("set executable bit");
}

#[cfg(not(unix))]
pub fn set_executable(_path: &Path) {}

pub fn json_envelope(stdout: &[u8]) -> Value {
    let text = String::from_utf8_lossy(stdout);
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| {
            panic!(
                "expected JSON envelope on stdout, got {} byte(s)",
                stdout.len()
            )
        });
    let value: Value = serde_json::from_str(line).expect("parse JSON envelope");
    assert!(
        value.get("ok").is_some(),
        "JSON envelope is missing `ok` (line_len={})",
        line.len()
    );
    assert!(
        value.get("schema_version").is_some(),
        "JSON envelope is missing `schema_version` (line_len={})",
        line.len()
    );
    value
}

pub fn assert_redacted(text: &str, secret: &str) {
    assert_no_secret_leak(text.as_bytes(), secret.as_bytes());
}

pub fn assert_no_secret_leak(haystack: &[u8], secret: &[u8]) {
    if secret.is_empty() {
        return;
    }
    assert!(
        !contains_bytes(haystack, secret),
        "secret leaked in output (output_len={}, secret_len={})",
        haystack.len(),
        secret.len()
    );
}

pub fn command_with_timeout(command: &mut Command, timeout: Duration) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn_guard(command);
    let deadline = Instant::now() + timeout;

    loop {
        if child
            .child_mut()
            .try_wait()
            .expect("poll child process")
            .is_some()
        {
            return child.wait_with_output();
        }

        if Instant::now() >= deadline {
            return child.kill_and_wait();
        }

        thread::sleep(Duration::from_millis(25));
    }
}

pub fn spawn_guard(command: &mut Command) -> ChildGuard {
    ChildGuard {
        child: Some(command.spawn().expect("spawn child process")),
    }
}

pub struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    pub fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("child already consumed")
    }

    pub fn wait_with_output(mut self) -> Output {
        self.child
            .take()
            .expect("child already consumed")
            .wait_with_output()
            .expect("wait for child output")
    }

    pub fn kill_and_wait(mut self) -> Output {
        let mut child = self.child.take().expect("child already consumed");
        let _ = child.kill();
        child
            .wait_with_output()
            .expect("wait for killed child output")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct HttpServer {
    addr: SocketAddr,
    child: ChildGuard,
}

impl HttpServer {
    pub fn start(workspace: &Path, token: &str, timeout: Duration) -> Self {
        let addr = localhost_addr();
        let mut command = omakure_command();
        command
            .arg("--scripts-dir")
            .arg(workspace)
            .arg("api")
            .arg("--bind")
            .arg(addr.to_string())
            .env("OMAKURE_API_TOKEN", token);

        let child = spawn_guard(&mut command);
        let server = Self { addr, child };
        server.wait_until_ready(timeout);
        server
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    fn wait_until_ready(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let url = self.url("/v1/health");
        while Instant::now() < deadline {
            if http_get_status(&url, None, Duration::from_millis(250)) == Some(200) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("HTTP server did not become ready within {timeout:?}");
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        let _ = self.child.child_mut().kill();
        let _ = self.child.child_mut().wait();
    }
}

pub fn http_get_with_timeout(url: &str, bearer_token: Option<&str>, timeout: Duration) -> String {
    let (_status, body) = http_get(url, bearer_token, timeout).expect("HTTP GET should succeed");
    body
}

fn http_get_status(url: &str, bearer_token: Option<&str>, timeout: Duration) -> Option<u16> {
    http_get(url, bearer_token, timeout)
        .ok()
        .map(|(status, _body)| status)
}

fn http_get(
    url: &str,
    bearer_token: Option<&str>,
    timeout: Duration,
) -> std::io::Result<(u16, String)> {
    let (addr, host, path) = parse_http_url(url);
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let auth = bearer_token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    Ok((status, body.to_string()))
}

fn parse_http_url(url: &str) -> (SocketAddr, String, String) {
    let rest = url
        .strip_prefix("http://")
        .expect("support HTTP client only accepts http:// URLs");
    let (host_port, path) = rest.split_once('/').unwrap_or((rest, ""));
    let addr: SocketAddr = host_port.parse().expect("parse host:port");
    (addr, host_port.to_string(), format!("/{path}"))
}

fn localhost_addr() -> SocketAddr {
    for _ in 0..1_000 {
        let port = NEXT_PORT.fetch_add(1, Ordering::SeqCst);
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpListener::bind(addr).is_ok() {
            return addr;
        }
    }
    panic!("no deterministic localhost test port available");
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
