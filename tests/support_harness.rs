mod support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn support_harness_provides_workspace_script_json_and_secret_assertions() {
    let workspace = support::TestWorkspace::new("harness_self_test");
    let script = workspace.write_schema_script("hello.sh", "hello", "echo fixture-ok");
    support::set_executable(&script);

    let output = support::command_with_timeout(
        support::omakure_command()
            .arg("--scripts-dir")
            .arg(workspace.path())
            .arg("--json")
            .arg("run")
            .arg("hello.sh"),
        Duration::from_secs(10),
    );

    assert!(
        output.status.success(),
        "expected run success, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = support::json_envelope(&output.stdout);
    assert_eq!(envelope["ok"], true);

    support::assert_redacted("token=[REDACTED]", "super-secret-token");
    support::assert_no_secret_leak(&output.stdout, b"super-secret-token");
}

#[test]
fn support_harness_provides_http_port_readiness_and_teardown() {
    let workspace = support::TestWorkspace::new("harness_http_self_test");
    let token = "harness-token-0123456789abcdef012345";
    let server = support::HttpServer::start(workspace.path(), token, Duration::from_secs(10));

    let body = support::http_get_with_timeout(
        &server.url("/v1/health"),
        Some(token),
        Duration::from_secs(5),
    );
    let envelope = support::json_envelope(body.as_bytes());
    assert_eq!(envelope["ok"], true);
}

/// A read timeout firing mid-response is not a clean close. The peer here goes
/// quiet for several multiples of the socket read timeout, so the reader is
/// forced through `WouldBlock` repeatedly, and then finishes normally. The
/// helper must return the *whole* response, not the prefix it held when the
/// first timeout fired.
#[test]
fn harness_http_read_survives_read_timeouts_mid_response() {
    const HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n";
    const TAIL: &str = "{\"ok\":true}\r\n";

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind stalling peer");
    let addr = listener.local_addr().expect("read stalling peer addr");

    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut request = [0u8; 1024];
        let _ = socket.read(&mut request);
        // Deliver a prefix, then stall well past the client's read timeout.
        socket.write_all(HEAD.as_bytes()).expect("write head");
        socket.flush().expect("flush head");
        thread::sleep(Duration::from_millis(600));
        socket.write_all(TAIL.as_bytes()).expect("write tail");
        socket.flush().expect("flush tail");
        // Dropping `socket` closes cleanly, which is the reader's EOF.
    });

    let mut stream = TcpStream::connect(addr).expect("connect stalling peer");
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set read timeout");
    stream
        .write_all(b"GET /v1/health HTTP/1.1\r\nConnection: close\r\n\r\n")
        .expect("write request");

    let raw = support::read_http_response(&mut stream, Duration::from_secs(10))
        .expect("a slow peer that closes cleanly must not be an error");
    server.join().expect("stalling peer thread");

    assert_eq!(
        raw,
        format!("{HEAD}{TAIL}"),
        "helper returned a truncated response instead of retrying past WouldBlock"
    );
    let response = support::HttpResponse::parse(raw);
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["ok"], true);
}

/// A peer that sends a prefix and then never closes is a genuine hang. The
/// helper must fail once its budget is spent — never hang forever, and never
/// hand back the truncated prefix as if it were the whole response.
#[test]
fn harness_http_read_fails_when_the_peer_never_closes() {
    const HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\n";

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind hanging peer");
    let addr = listener.local_addr().expect("read hanging peer addr");

    let (release, released) = mpsc::channel::<()>();
    thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept");
        let mut request = [0u8; 1024];
        let _ = socket.read(&mut request);
        socket.write_all(HEAD.as_bytes()).expect("write head");
        socket.flush().expect("flush head");
        // Hold the connection open — no body, no close — until the test is done.
        let _ = released.recv_timeout(Duration::from_secs(30));
    });

    let mut stream = TcpStream::connect(addr).expect("connect hanging peer");
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set read timeout");
    stream
        .write_all(b"GET /v1/health HTTP/1.1\r\nConnection: close\r\n\r\n")
        .expect("write request");

    let started = Instant::now();
    let error = support::read_http_response(&mut stream, Duration::from_millis(750))
        .expect_err("a peer that never closes must fail, not return a truncated body");
    let elapsed = started.elapsed();
    drop(release);

    assert_eq!(
        error.kind(),
        std::io::ErrorKind::TimedOut,
        "a hang must be reported as a timeout, got {error}"
    );
    let message = error.to_string();
    assert!(
        message.contains("truncated"),
        "the failure must say the response is truncated, got: {message}"
    );
    assert!(
        message.contains(&format!("{} byte(s)", HEAD.len())),
        "the failure must report how much arrived, got: {message}"
    );
    assert!(
        elapsed >= Duration::from_millis(750),
        "helper gave up before its budget was spent: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "helper did not bound the hang: {elapsed:?}"
    );
}
