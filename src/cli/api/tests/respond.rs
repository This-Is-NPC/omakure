use super::*;

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
