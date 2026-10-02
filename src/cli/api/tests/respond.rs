use super::*;
use serde::ser::Serializer;
use std::sync::Condvar;
use std::time::{Duration, Instant};

#[tokio::test]
async fn unsupported_discovery_platform_maps_to_501() {
    let response = operation_error_response(OperationError::new(
        OperationErrorCode::DiscoveryUnsupportedPlatform,
        "discovery is unsupported on this platform",
    ));

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "discovery_unsupported_platform");
}

#[tokio::test]
async fn malformed_json_returns_envelope() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);

    let response = router(workspace)
        .oneshot(authed_json_request("/v1/runs", r#"{"#))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response_json(response).await;
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "invalid_input");
}

#[tokio::test]
async fn oversized_json_returns_envelope() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let body = format!(r#"{{"script":"{}"}}"#, "x".repeat(BODY_LIMIT_BYTES + 1));

    let response = router(workspace)
        .oneshot(authed_json_request("/v1/runs", &body))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = response_json(response).await;
    assert_eq!(body["error"]["code"], "payload_too_large");
}

struct SlowResponse {
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    released: Arc<(Mutex<bool>, Condvar)>,
}

impl serde::Serialize for SlowResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(started) = self.started.lock().unwrap().take() {
            let _ = started.send(());
        }
        let (lock, ready) = &*self.released;
        let mut released = lock.lock().unwrap();
        while !*released {
            released = ready.wait(released).unwrap();
        }
        serializer.serialize_str("complete")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_response_serialization_does_not_stall_health() {
    let dir = TempDir::new().unwrap();
    let workspace = crate::test_support::workspace_in(&dir);
    let app = router(workspace);
    let gate = Arc::new(tokio::sync::Semaphore::new(1));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let response = SlowResponse {
        started: Mutex::new(Some(started_tx)),
        released: Arc::clone(&released),
    };
    let start = Instant::now();
    let pending = tokio::spawn(async move {
        super::super::blocking::operation_response_bounded("probe", gate, move || Ok(response))
            .await
    });

    let (watchdog_tx, watchdog_rx) = std::sync::mpsc::channel::<()>();
    let watchdog = std::thread::spawn(move || {
        let _ = watchdog_rx.recv_timeout(Duration::from_secs(3));
        let (lock, ready) = &*released;
        *lock.lock().unwrap() = true;
        ready.notify_all();
    });

    let started = tokio::time::timeout(Duration::from_secs(2), started_rx).await;
    let health = tokio::time::timeout(
        Duration::from_secs(1),
        app.oneshot(authed_request("/v1/health")),
    )
    .await;
    let elapsed = start.elapsed();
    let _ = watchdog_tx.send(());
    watchdog.join().unwrap();

    started.unwrap().unwrap();
    assert_eq!(health.unwrap().unwrap().status(), StatusCode::OK);
    assert!(elapsed < Duration::from_secs(2));
    assert_eq!(pending.await.unwrap().status(), StatusCode::OK);
}
