use std::collections::HashMap;

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Valid variable name: `[A-Za-z_][A-Za-z0-9_]*`.
pub(super) fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if is_name_start(first) => chars.all(is_name_char),
        _ => false,
    }
}

/// Single-pass, non-recursive `$VAR` / `${VAR}` expansion per the
/// env-injection grammar (`docs/internal/env-injection-spec.md` section 2).
///
/// - The input is scanned left-to-right exactly once; substituted output is
///   never re-scanned (no recursion).
/// - `$VAR` bare form: the name is the longest run of `[A-Za-z0-9_]` after a
///   name-start (`[A-Za-z_]`).
/// - `${VAR}` braced form: the body between `{` and the next `}`. A body that
///   is not a valid name resolves as undefined. An unterminated `${...` is
///   emitted literally.
/// - Undefined references expand to the empty string.
/// - The only escape is `\$` -> literal `$`; any other `\` is literal.
/// - No command substitution: `$(...)` and backticks are emitted literally.
pub(super) fn expand_env_value(input: &str, vars: &HashMap<String, String>) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '\\' if chars.get(i + 1) == Some(&'$') => {
                out.push('$');
                i += 2;
            }
            '$' => match append_reference(&chars, i, vars, &mut out) {
                Some(next) => i = next,
                None => {
                    out.extend(chars[i..].iter());
                    break;
                }
            },
            c => {
                out.push(c);
                i += 1;
            }
        }
    }

    out
}

fn append_reference(
    chars: &[char],
    dollar: usize,
    vars: &HashMap<String, String>,
    out: &mut String,
) -> Option<usize> {
    let next = dollar + 1;
    if chars.get(next) == Some(&'{') {
        let close = (next + 1..chars.len()).find(|&index| chars[index] == '}')?;
        let name: String = chars[next + 1..close].iter().collect();
        if is_valid_var_name(&name) {
            append_variable(&name, vars, out);
        }
        return Some(close + 1);
    }
    if chars.get(next).is_some_and(|&c| is_name_start(c)) {
        let mut end = next + 1;
        while end < chars.len() && is_name_char(chars[end]) {
            end += 1;
        }
        let name: String = chars[next..end].iter().collect();
        append_variable(&name, vars, out);
        return Some(end);
    }
    out.push('$');
    Some(next)
}

fn append_variable(name: &str, vars: &HashMap<String, String>, out: &mut String) {
    out.push_str(vars.get(name).map(String::as_str).unwrap_or(""));
}

pub(super) fn strip_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2 {
        let first = trimmed.as_bytes()[0] as char;
        let last = trimmed.as_bytes()[trimmed.len() - 1] as char;
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

pub(crate) fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    let tokens = [
        "password",
        "passwd",
        "pwd",
        "secret",
        "token",
        "key",
        "api",
        "private",
        "cred",
        "passphrase",
        "auth",
        "bearer",
    ];
    tokens.iter().any(|token| lower.contains(token))
}

pub(crate) fn value_contains_credentials(value: &str) -> bool {
    let Some(scheme_end) = value.find("://") else {
        return false;
    };
    if scheme_end == 0 {
        return false;
    }
    let authority = &value[scheme_end + 3..];
    let authority_end = authority.find(['/', '?', '#']).unwrap_or(authority.len());
    let authority = &authority[..authority_end];
    let Some(at) = authority.rfind('@') else {
        return false;
    };
    let userinfo = &authority[..at];
    let Some(colon) = userinfo.find(':') else {
        return false;
    };
    colon > 0 && colon + 1 < userinfo.len()
}

/// Decide whether a managed-env value should be masked (`****`) on read paths
/// (`env show`, `GET /v1/envs/:name`).
///
/// This is a best-effort **denylist heuristic** — it masks values whose key
/// looks sensitive ([`is_sensitive_key`]) or whose value embeds URL
/// credentials ([`value_contains_credentials`]). It CANNOT catch a real secret
/// stored under an innocuous key with an opaque value (e.g.
/// `DEPLOY_HOOK=T00xxxxSECRET`); such a value is returned in cleartext.
///
/// Managed envs are config, not a secret store: operators must put real
/// secrets behind `secret://` refs (env/file providers), which the redaction
/// pipeline covers end-to-end at rest and in output, rather than relying on
/// this display mask.
pub(crate) fn should_mask_env_value(key: &str, value: &str) -> bool {
    !value.is_empty() && (is_sensitive_key(key) || value_contains_credentials(value))
}
