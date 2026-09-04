#![allow(dead_code)]

use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::process::{Child, Command, Output};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

static RESERVED_TEST_PORTS: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();

pub fn unique_loopback_port() -> u16 {
    let ports = RESERVED_TEST_PORTS.get_or_init(|| Mutex::new(HashSet::new()));
    loop {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test port");
        let port = listener.local_addr().expect("read test port").port();
        if ports.lock().expect("lock test ports").insert(port) {
            return port;
        }
    }
}

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
    let stdout = child.child_mut().stdout.take().expect("child stdout pipe");
    let stderr = child.child_mut().stderr.take().expect("child stderr pipe");
    // Drain both pipes while the child runs. Waiting for the child before
    // reading either pipe deadlocks when successful output exceeds pipe
    // capacity (for example, `help-ai`).
    let stdout_reader = thread::spawn(move || read_child_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_child_pipe(stderr));
    let deadline = Instant::now() + timeout;

    let status = loop {
        if let Some(status) = child.child_mut().try_wait().expect("poll child process") {
            break status;
        }

        if Instant::now() >= deadline {
            let process = child.child_mut();
            let _ = process.kill();
            break process.wait().expect("wait for killed child");
        }

        thread::sleep(Duration::from_millis(25));
    };
    // The process has been reaped; remove it from the guard so Drop does not
    // attempt to kill it again.
    let _ = child.take_child();

    Output {
        status,
        stdout: stdout_reader.join().expect("read child stdout"),
        stderr: stderr_reader.join().expect("read child stderr"),
    }
}

fn read_child_pipe(mut pipe: impl Read) -> Vec<u8> {
    let mut output = Vec::new();
    pipe.read_to_end(&mut output).expect("read child output");
    output
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

    pub fn take_child(&mut self) -> Option<Child> {
        self.child.take()
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
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    pub fn parse(raw: String) -> Self {
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("parse HTTP status");
        Self {
            status,
            body: body.to_string(),
        }
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("parse HTTP JSON body")
    }

    pub fn safe_body(&self) -> String {
        format!("{} byte response", self.body.len())
    }

    pub fn assert_no_secret(&self, secret: &str) {
        assert_redacted(&self.body, secret);
    }
}

pub struct HttpServer {
    addr: SocketAddr,
    child: ChildGuard,
    token: String,
}

impl HttpServer {
    pub fn start(workspace: &Path, token: &str, timeout: Duration) -> Self {
        Self::start_with_args(workspace, token, &[], &[], timeout)
    }

    pub fn start_with_args(
        workspace: &Path,
        token: &str,
        extra_args: &[&str],
        extra_envs: &[(&str, &str)],
        timeout: Duration,
    ) -> Self {
        Self::start_command("api", workspace, token, extra_args, extra_envs, timeout)
    }

    pub fn start_node_service(
        workspace: &Path,
        token: &str,
        extra_args: &[&str],
        extra_envs: &[(&str, &str)],
        timeout: Duration,
    ) -> Self {
        Self::start_command("node", workspace, token, extra_args, extra_envs, timeout)
    }

    fn start_command(
        command_name: &str,
        workspace: &Path,
        token: &str,
        extra_args: &[&str],
        extra_envs: &[(&str, &str)],
        timeout: Duration,
    ) -> Self {
        // Reserve a process-wide unique port before spawning. Unlike probing with
        // a temporary listener here, this keeps sibling test servers from choosing
        // the same port during their bind→spawn window.
        let attempt_timeout = timeout / 4;
        let attempt_timeout = if attempt_timeout.is_zero() {
            Duration::from_secs(2)
        } else {
            attempt_timeout.max(Duration::from_millis(500))
        };
        let deadline = Instant::now() + timeout;
        let mut last_addr = None;
        while Instant::now() < deadline {
            let addr = SocketAddr::from(([127, 0, 0, 1], unique_loopback_port()));
            last_addr = Some(addr);
            let mut command = omakure_command();
            command
                .arg("--scripts-dir")
                .arg(workspace)
                .arg(command_name)
                .args((command_name == "node").then_some("serve"))
                .arg("--bind")
                .arg(addr.to_string())
                .args(extra_args)
                .env("OMAKURE_API_TOKEN", token);
            if command_name == "node" {
                command
                    .env("OMAKURE_NODE_TEST_MODE", "1")
                    .env("OMAKURE_NODE_STATE_DIR", workspace.join(".node-state"))
                    .env("OMAKURE_NODE_CONFIG", workspace.join("node.toml"));
            }
            for (key, value) in extra_envs {
                command.env(key, value);
            }

            let child = spawn_guard(&mut command);
            let mut server = Self {
                addr,
                child,
                token: token.to_string(),
            };
            if server.try_wait_until_ready(
                attempt_timeout,
                if command_name == "node" {
                    "/v1/ready"
                } else {
                    "/v1/health"
                },
            ) {
                return server;
            }
            let _ = server.child.child_mut().kill();
            let _ = server.child.child_mut().wait();
            thread::sleep(Duration::from_millis(25));
        }
        panic!(
            "HTTP {command_name} did not become ready within {timeout:?} (last_addr={last_addr:?})"
        );
    }

    pub fn child_id(&mut self) -> u32 {
        self.child.child_mut().id()
    }

    pub fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child
            .child_mut()
            .try_wait()
            .expect("poll node-service child")
    }

    pub fn wait_exit(mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait() {
                // Prevent Drop from killing an already-reaped child.
                let _ = self.child.take_child();
                return status;
            }
            if Instant::now() >= deadline {
                let mut child = self.child.take_child().expect("child already consumed");
                let _ = child.kill();
                return child.wait().expect("wait for killed child");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn terminate(mut self) -> std::process::ExitStatus {
        let mut child = self.child.take_child().expect("child already consumed");
        child.kill().expect("terminate child process");
        child.wait().expect("wait for terminated child")
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn get(&self, path: &str) -> HttpResponse {
        self.request("GET", path, None)
    }

    pub fn post_json(&self, path: &str, body: &Value) -> HttpResponse {
        self.request("POST", path, Some(body.to_string()))
    }

    pub fn put_json(&self, path: &str, body: &Value) -> HttpResponse {
        self.request("PUT", path, Some(body.to_string()))
    }

    pub fn patch_json(&self, path: &str, body: &Value) -> HttpResponse {
        self.request("PATCH", path, Some(body.to_string()))
    }

    pub fn delete(&self, path: &str) -> HttpResponse {
        self.request("DELETE", path, None)
    }

    pub fn get_unauthenticated(&self, path: &str) -> HttpResponse {
        self.request_with_auth("GET", path, None, AuthMode::None)
    }

    pub fn get_with_bearer(&self, path: &str, token: &str) -> HttpResponse {
        self.request_with_auth("GET", path, None, AuthMode::Bearer(token))
    }

    /// Poll `/v1/ready` until the service reports semantic readiness.
    ///
    /// Node-service startup performs identity, registry/schema, and transport
    /// setup before this endpoint is allowed to return 200. Worker, scheduler,
    /// and transport requirements are then reflected by the same gate, so a
    /// successful probe is safe for callers to use immediately.
    ///
    /// A service that never becomes ready still fails the test: once the
    /// deadline passes the last response is returned as-is, so the caller's
    /// assertion reports the real status and body rather than hanging or
    /// silently passing.
    pub fn await_ready(&self, timeout: Duration) -> HttpResponse {
        let deadline = Instant::now() + timeout;
        loop {
            let response = self.get_unauthenticated("/v1/ready");
            if response.status == 200 || Instant::now() >= deadline {
                return response;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn request(&self, method: &str, path: &str, body: Option<String>) -> HttpResponse {
        self.request_with_auth(method, path, body, AuthMode::Bearer(&self.token))
    }

    pub fn request_with_auth(
        &self,
        method: &str,
        path: &str,
        body: Option<String>,
        auth: AuthMode<'_>,
    ) -> HttpResponse {
        let timeout = Duration::from_secs(5);
        let mut stream = TcpStream::connect_timeout(&self.addr, timeout).expect("connect HTTP API");
        stream
            .set_read_timeout(Some(timeout))
            .expect("set read timeout");
        stream
            .set_write_timeout(Some(timeout))
            .expect("set write timeout");

        let body = body.unwrap_or_default();
        let content_headers = if matches!(method, "POST" | "PUT" | "PATCH") {
            format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            )
        } else {
            String::new()
        };
        let auth_header = match auth {
            AuthMode::None => String::new(),
            AuthMode::Bearer(token) => format!("Authorization: Bearer {token}\r\n"),
        };
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\n{auth_header}{content_headers}Connection: close\r\n\r\n{body}",
            self.addr
        );
        stream.write_all(request.as_bytes()).expect("write request");

        let raw = read_http_response(&mut stream, HTTP_RESPONSE_DEADLINE)
            .unwrap_or_else(|error| panic!("read response from {}: {error}", self.addr));
        HttpResponse::parse(raw)
    }

    fn try_wait_until_ready(&mut self, timeout: Duration, path: &str) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            // A successful probe is only ours if the process that was spawned
            // for this address is still alive. This rejects a 200 from a
            // different responder after our child lost the bind race.
            if self
                .child
                .child_mut()
                .try_wait()
                .expect("poll HTTP child")
                .is_some()
            {
                return false;
            }
            if let Ok(mut stream) =
                TcpStream::connect_timeout(&self.addr, Duration::from_millis(200))
            {
                let request = format!(
                    "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                    self.addr
                );
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                if stream.write_all(request.as_bytes()).is_ok() {
                    let mut raw = String::new();
                    if stream.read_to_string(&mut raw).is_ok() && raw.contains(" 200 ") {
                        return self
                            .child
                            .child_mut()
                            .try_wait()
                            .expect("poll HTTP child after health probe")
                            .is_none();
                    }
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
        false
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        // ChildGuard::drop kills if still present; no-op when wait_exit took it.
    }
}

/// Exit status after an intentional test-harness kill.
/// Unix SIGKILL reports no exit code; Windows TerminateProcess uses code 1.
pub fn terminated_exit_is_expected(status: std::process::ExitStatus) -> bool {
    terminated_exit_is_expected_for_parts(status.success(), status.code())
}

pub fn terminated_exit_is_expected_for_parts(success: bool, code: Option<i32>) -> bool {
    success || code.is_none() || (cfg!(windows) && code == Some(1))
}

pub fn assert_terminated(status: std::process::ExitStatus) {
    assert!(
        terminated_exit_is_expected(status),
        "expected graceful exit or test-harness kill, got {status:?}"
    );
}

/// Overall budget for one HTTP exchange made through [`HttpServer`].
///
/// Deliberately far larger than the per-read socket timeout. `set_read_timeout`
/// bounds a *single* `read` syscall, not the exchange: an Argon2id verification
/// (64 MiB, t=3) on a machine running the whole suite in parallel can leave the
/// socket idle for several seconds while the peer is perfectly healthy.
const HTTP_RESPONSE_DEADLINE: Duration = Duration::from_secs(60);

/// Read an HTTP response until the peer closes, tolerating read-timeout stalls.
///
/// Three outcomes must stay distinct, and conflating any two of them hides a
/// real defect:
///
/// * the peer closed cleanly (`Ok(0)`) — the response is complete, return it;
/// * a single `read` timed out (`WouldBlock`/`TimedOut`) — the peer is merely
///   slow, so retry until `deadline`;
/// * `deadline` passed without a clean close — a genuine hang, so fail with the
///   byte count so the truncation is visible in the panic.
///
/// Retrying rather than ignoring the error is the whole point. `read_to_string`
/// *keeps* the bytes it already consumed when it fails, so downgrading the
/// error to `let _ = ...` would return a silently truncated response that still
/// parses as valid HTTP — turning a real hang into a green assertion.
pub fn read_http_response(stream: &mut TcpStream, deadline: Duration) -> std::io::Result<String> {
    use std::io::ErrorKind;

    let give_up = Instant::now() + deadline;
    let mut raw: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                if Instant::now() >= give_up {
                    return Err(std::io::Error::new(
                        ErrorKind::TimedOut,
                        format!(
                            "peer never closed the connection within {deadline:?}; \
                             {} byte(s) received so far (response is truncated)",
                            raw.len()
                        ),
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    }
    String::from_utf8(raw)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub fn http_get_with_timeout(url: &str, bearer_token: Option<&str>, timeout: Duration) -> String {
    let (_status, body) = http_get(url, bearer_token, timeout).expect("HTTP GET should succeed");
    body
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

    let response = read_http_response(&mut stream, timeout)?;
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

pub enum AuthMode<'a> {
    None,
    Bearer(&'a str),
}

/// Git `-c` flags that keep battery cache checkouts byte-identical across platforms.
pub fn battery_cache_git_config_args() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["-c", "core.autocrlf=false", "-c", "core.filemode=false"]
    }
    #[cfg(not(windows))]
    {
        &["-c", "core.autocrlf=false"]
    }
}

/// Write a local git battery fixture under `root` and return after the initial commit.
pub fn write_local_battery_repo(root: &Path, battery_name: &str, description: &str) {
    fs::create_dir_all(root.join("scripts")).expect("create battery scripts dir");
    fs::write(
        root.join("omakure-battery.toml"),
        format!(
            r#"[battery]
name = "{battery_name}"
version = "0.1.0"
description = "{description}"

[[scripts]]
id = "local.echo"
path = "scripts/echo.sh"
description = "Echo fixture"
tags = ["test"]
"#
        ),
    )
    .expect("write manifest");
    fs::write(
        root.join("scripts/echo.sh"),
        r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {"Name":"Battery Echo","Description":"Echo fixture","Fields":[]}
# OMAKURE_SCHEMA_END
echo battery
"#,
    )
    .expect("write battery script");
    set_executable(&root.join("scripts/echo.sh"));
    run_git(root, &["init", "-b", "main"]);
    run_git(root, &["add", "."]);
    run_git(
        root,
        &[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Test User",
            "commit",
            "-m",
            "battery fixture",
        ],
    );
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn git");
    assert!(
        output.status.success(),
        "git {:?} failed (stderr_len={})",
        args,
        output.stderr.len()
    );
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
