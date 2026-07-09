mod support;

use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::Output;
use std::time::{Duration, Instant};

const API_TOKEN: &str = "http-api-e2e-token-with-enough-entropy-0001";
const SECRET_DEFAULT: &str = "http-schema-secret-default-plain-value";
const QUEUE_SECRET: &str = "http-queue-secret-provider-plain-value";

#[test]
fn runs_post_is_forbidden_without_write_capability() {
    let workspace = support::TestWorkspace::new("http_deny_runs");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = HttpServer::start(
        workspace.path(),
        API_TOKEN,
        &["--capability", "scripts:read"],
        Duration::from_secs(10),
    );

    let response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", "secret://env/OMAKURE_HTTP_QUEUE_TOKEN"]
        }),
    );

    assert_eq!(response.status, 403, "body: {}", response.safe_body());
    assert_error_code(&response.json(), "forbidden");
}

#[test]
fn script_schema_redacts_secret_defaults_over_http() {
    let workspace = support::TestWorkspace::new("http_schema_redact");
    write_secret_echo_script(workspace.path(), "secret-default.sh", Some(SECRET_DEFAULT));
    let server = HttpServer::start(
        workspace.path(),
        API_TOKEN,
        &["--capability", "scripts:read"],
        Duration::from_secs(10),
    );

    let response = server.get("/v1/scripts/secret-default.sh/schema");

    assert_eq!(response.status, 200, "body: {}", response.safe_body());
    response.assert_no_secret(SECRET_DEFAULT);
    let envelope = response.json();
    let fields = envelope["data"]["fields"]
        .as_array()
        .expect("schema fields");
    let token = fields
        .iter()
        .find(|field| field["name"] == "TOKEN")
        .expect("TOKEN field");
    assert_eq!(token["default"], Value::Null);
}

#[test]
fn runs_post_rejects_plaintext_secret_fields_and_secret_args() {
    let workspace = support::TestWorkspace::new("http_reject_plaintext");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = HttpServer::start(
        workspace.path(),
        API_TOKEN,
        &["--capability", "runs:write", "--capability", "secrets:use"],
        Duration::from_secs(10),
    );

    let secret_field_response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "secret_fields": { "TOKEN": QUEUE_SECRET }
        }),
    );
    assert_eq!(
        secret_field_response.status,
        400,
        "body: {}",
        secret_field_response.safe_body()
    );
    secret_field_response.assert_no_secret(QUEUE_SECRET);
    assert_error_contains(&secret_field_response.json(), "secret://");

    let args_response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", QUEUE_SECRET]
        }),
    );
    assert_eq!(
        args_response.status,
        400,
        "body: {}",
        args_response.safe_body()
    );
    args_response.assert_no_secret(QUEUE_SECRET);
    assert_error_contains(&args_response.json(), "secret://");
}

#[test]
fn authorized_secret_ref_enqueue_worker_and_history_do_not_leak_secret() {
    let workspace = support::TestWorkspace::new("http_secret_queue");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = HttpServer::start(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "runs:read",
            "--capability",
            "runs:write",
            "--capability",
            "secrets:use",
            "--secret-ref",
            "secret://env/OMAKURE_HTTP_QUEUE_TOKEN",
        ],
        Duration::from_secs(10),
    );

    let enqueue = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", "secret://env/OMAKURE_HTTP_QUEUE_TOKEN"]
        }),
    );
    assert_eq!(enqueue.status, 200, "body: {}", enqueue.safe_body());
    enqueue.assert_no_secret(QUEUE_SECRET);
    let run_id = enqueue.json()["data"]["run_id"]
        .as_str()
        .expect("run id")
        .to_string();

    let worker = omakure_with_env(
        workspace.path(),
        &["--json", "queue", "worker", "--once"],
        &[
            ("OMAKURE_HTTP_QUEUE_TOKEN", QUEUE_SECRET),
            ("OMAKURE_EXPECTED_TOKEN", QUEUE_SECRET),
        ],
    );
    assert_success(&worker);
    assert_no_plaintext(&worker, QUEUE_SECRET);

    let show = server.get(&format!("/v1/runs/{run_id}"));
    assert_eq!(show.status, 200, "body: {}", show.safe_body());
    show.assert_no_secret(QUEUE_SECRET);
    let show_json = show.json();
    assert_eq!(show_json["data"]["state"], "completed");
    assert_eq!(show_json["data"]["stdout"], "script-saw-redacted-ok\n");

    let history = server.get("/v1/runs");
    assert_eq!(history.status, 200, "body: {}", history.safe_body());
    history.assert_no_secret(QUEUE_SECRET);
    let serialized_history = serde_json::to_string(&history.json()).expect("serialize history");
    assert!(serialized_history.contains(&run_id));
    assert!(serialized_history.contains("script-saw-redacted-ok"));
}

#[test]
fn unauthorized_secret_provider_ref_is_forbidden_without_leaking_secret() {
    let workspace = support::TestWorkspace::new("http_secret_ref_deny");
    write_secret_echo_script(workspace.path(), "secret-echo.sh", None);
    let server = HttpServer::start(
        workspace.path(),
        API_TOKEN,
        &[
            "--capability",
            "runs:write",
            "--capability",
            "secrets:use",
            "--secret-ref",
            "secret://env/ALLOWED_ONLY",
        ],
        Duration::from_secs(10),
    );

    let response = server.post_json(
        "/v1/runs",
        &json!({
            "script": "secret-echo.sh",
            "args": ["--token", "secret://env/OMAKURE_HTTP_QUEUE_TOKEN"]
        }),
    );

    assert_eq!(response.status, 403, "body: {}", response.safe_body());
    response.assert_no_secret(QUEUE_SECRET);
    assert!(!response.body.contains("OMAKURE_HTTP_QUEUE_TOKEN"));
    assert_error_code(&response.json(), "forbidden");
}

fn write_secret_echo_script(workspace: &Path, name: &str, default: Option<&str>) {
    let default_line = default
        .map(|value| format!(r#", "Default":"{value}""#))
        .unwrap_or_default();
    let script = workspace.join(name);
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
# OMAKURE_SCHEMA_START
# {{
#   "Name": "secret_echo",
#   "Description": "secret HTTP e2e fixture",
#   "Fields": [
#     {{"Name":"TOKEN","Prompt":"Token","Type":"secret","Required":true,"Arg":"--token"{default_line}}}
#   ]
# }}
# OMAKURE_SCHEMA_END
if [ "$2" = "$OMAKURE_EXPECTED_TOKEN" ]; then
  printf 'script-saw-redacted-ok\n'
else
  printf 'secret mismatch\n' >&2
  exit 42
fi
"#
        ),
    )
    .expect("write secret script");
    support::set_executable(&script);
}

fn omakure_with_env(workspace: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = support::omakure_command();
    command.arg("--scripts-dir").arg(workspace).args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    support::command_with_timeout(&mut command, Duration::from_secs(15))
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success, status: {:?}, stdout_len: {}, stderr_len: {}",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len()
    );
}

fn assert_no_plaintext(output: &Output, secret: &str) {
    support::assert_no_secret_leak(&output.stdout, secret.as_bytes());
    support::assert_no_secret_leak(&output.stderr, secret.as_bytes());
}

fn assert_error_contains(envelope: &Value, needle: &str) {
    let message = envelope["error"]["message"]
        .as_str()
        .expect("error message");
    assert!(
        message.contains(needle),
        "unexpected error message: {message}"
    );
}

fn assert_error_code(envelope: &Value, expected: &str) {
    assert_eq!(envelope["error"]["code"], expected);
}

struct HttpServer {
    addr: SocketAddr,
    child: support::ChildGuard,
}

impl HttpServer {
    fn start(workspace: &Path, token: &str, extra_args: &[&str], timeout: Duration) -> Self {
        let addr = ephemeral_loopback_addr();
        let mut command = support::omakure_command();
        command
            .arg("--scripts-dir")
            .arg(workspace)
            .arg("api")
            .arg("--bind")
            .arg(addr.to_string())
            .args(extra_args)
            .env("OMAKURE_API_TOKEN", token)
            .env("OMAKURE_HTTP_QUEUE_TOKEN", QUEUE_SECRET)
            .env("OMAKURE_EXPECTED_TOKEN", QUEUE_SECRET);
        let child = support::spawn_guard(&mut command);
        let server = Self { addr, child };
        server.wait_until_ready(timeout);
        server
    }

    fn get(&self, path: &str) -> HttpResponse {
        self.request("GET", path, None)
    }

    fn post_json(&self, path: &str, body: &Value) -> HttpResponse {
        self.request("POST", path, Some(body.to_string()))
    }

    fn request(&self, method: &str, path: &str, body: Option<String>) -> HttpResponse {
        let timeout = Duration::from_secs(5);
        let mut stream = TcpStream::connect_timeout(&self.addr, timeout).expect("connect HTTP API");
        stream
            .set_read_timeout(Some(timeout))
            .expect("set read timeout");
        stream
            .set_write_timeout(Some(timeout))
            .expect("set write timeout");

        let body = body.unwrap_or_default();
        let headers = if method == "POST" {
            format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                body.len()
            )
        } else {
            String::new()
        };
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {API_TOKEN}\r\n{headers}Connection: close\r\n\r\n{body}",
            self.addr
        );
        stream.write_all(request.as_bytes()).expect("write request");

        let mut raw = String::new();
        stream.read_to_string(&mut raw).expect("read response");
        HttpResponse::parse(raw)
    }

    fn wait_until_ready(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(mut stream) =
                TcpStream::connect_timeout(&self.addr, Duration::from_millis(200))
            {
                let request = format!(
                    "GET /v1/health HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {API_TOKEN}\r\nConnection: close\r\n\r\n",
                    self.addr
                );
                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                if stream.write_all(request.as_bytes()).is_ok() {
                    let mut raw = String::new();
                    if stream.read_to_string(&mut raw).is_ok() && raw.contains(" 200 ") {
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(25));
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

struct HttpResponse {
    status: u16,
    body: String,
}

impl HttpResponse {
    fn parse(raw: String) -> Self {
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

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("parse HTTP JSON body")
    }

    fn safe_body(&self) -> String {
        format!("{} byte response", self.body.len())
    }

    fn assert_no_secret(&self, secret: &str) {
        support::assert_redacted(&self.body, secret);
    }
}

fn ephemeral_loopback_addr() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral localhost port");
    listener.local_addr().expect("read local addr")
}
