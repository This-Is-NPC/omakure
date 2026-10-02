pub const WILDCARD_SCOPE: &str = "*";

/// Whether `granted` scopes satisfy `required`.
///
/// Supports `*`, exact match, env name aliases (`env:read` ↔ `envs:read`), and
/// **one-way** coarse→fine coverage (`runs:write` covers `runs:enqueue`, but
/// `runs:enqueue` does **not** satisfy a `runs:write` check). Fine scopes must
/// never escalate to coarser write privileges.
pub fn scope_allows(granted: &[String], required: &str) -> bool {
    if granted.iter().any(|s| s == WILDCARD_SCOPE) {
        return true;
    }
    for g in granted {
        if scopes_match(g, required) {
            return true;
        }
    }
    false
}

fn scopes_match(granted: &str, required: &str) -> bool {
    if normalize_scope(granted) == normalize_scope(required) {
        return true;
    }
    // Coarse grants cover finer required actions only (never the reverse).
    matches!(
        (granted, required),
        (
            "runs:write",
            "runs:enqueue" | "runs:cancel" | "runs:dead-letter" | "runs:write"
        ) | (
            "batteries:write",
            "batteries:add"
                | "batteries:sync"
                | "batteries:install"
                | "batteries:remove"
                | "batteries:write",
        ) | (
            "config:read",
            "config:read" | "doctor:read" | "workspace:read"
        ) | ("scripts:read", "scripts:read" | "search:read")
    )
}

fn normalize_scope(scope: &str) -> &str {
    match scope {
        "env:read" | "envs:read" => "envs:read",
        "env:write" | "envs:write" => "envs:write",
        "env:activate" | "envs:activate" => "envs:activate",
        "env:use" | "envs:use" => "envs:use",
        other => other,
    }
}
