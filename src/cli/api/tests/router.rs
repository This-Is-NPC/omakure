use super::*;

#[test]
fn http_route_inventory_is_non_empty_and_unique() {
    assert!(!HTTP_ROUTE_INVENTORY.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for entry in HTTP_ROUTE_INVENTORY {
        assert!(
            seen.insert(*entry),
            "duplicate HTTP route inventory entry: {entry:?}"
        );
    }
}

#[test]
fn http_route_inventory_matches_router_with_policy_registrations() {
    let source = include_str!("../router.rs");
    let mut from_router = parse_router_route_registrations(source);
    // Health-plane routes register on `health_plane_router` and nest under `/v1/node`.
    from_router.extend([("GET", "/v1/node/health"), ("GET", "/v1/node/signals")]);
    from_router.sort();
    let mut inventory: Vec<_> = HTTP_ROUTE_INVENTORY.to_vec();
    inventory.sort();
    assert_eq!(
        from_router, inventory,
        "management API route registrations must equal HTTP_ROUTE_INVENTORY"
    );
}

fn parse_router_route_registrations(source: &str) -> Vec<(&str, &str)> {
    let start = source
        .find("fn router_with_policy(")
        .expect("router_with_policy");
    let after = &source[start..];
    let router_start = after.find("Router::new()").expect("Router::new");
    let block = &after[router_start..];
    // End at `.fallback(` which always follows the last `.route(...)`.
    let end = block.find(".fallback(").expect(".fallback after routes");
    let block = &block[..end];
    let mut routes = Vec::new();
    let mut i = 0;
    let bytes = block.as_bytes();
    while i < bytes.len() {
        if block[i..].starts_with(".route(") {
            i += ".route(".len();
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            assert_eq!(
                bytes.get(i),
                Some(&b'"'),
                "expected path string after .route("
            );
            i += 1;
            let path_start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let path = &block[path_start..i];
            i += 1; // closing quote
            while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
                i += 1;
            }
            let mut depth = 1usize;
            let methods_start = i;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                if depth > 0 {
                    i += 1;
                }
            }
            let methods_src = &block[methods_start..i];
            for (token, method) in [
                ("get(", "GET"),
                ("post(", "POST"),
                ("put(", "PUT"),
                ("patch(", "PATCH"),
                ("delete(", "DELETE"),
            ] {
                if methods_src.contains(token) {
                    routes.push((method, path));
                }
            }
        } else {
            i += 1;
        }
    }
    assert!(
        !routes.is_empty(),
        "parsed zero .route() registrations from router_with_policy"
    );
    routes
}
