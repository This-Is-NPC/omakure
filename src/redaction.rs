pub fn redact_secret(input: &str, secret: &str) -> String {
    if secret.is_empty() {
        return input.to_string();
    }

    let mut forms = vec![
        secret.to_string(),
        json_slash_escaped(secret),
        json_slash_escaped(secret).replace("\\/", "\\\\/"),
        url_encode(secret),
        url_encode_with_lowercase_escapes(secret),
    ];
    if let Some(json_escaped) = json_escaped_inner(secret) {
        forms.push(json_escaped);
    }
    forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
    forms.dedup();

    let mut redacted = input.to_string();
    for form in forms {
        redacted = redacted.replace(&form, "<redacted>");
    }
    redacted
}

fn json_escaped_inner(value: &str) -> Option<String> {
    let encoded = serde_json::to_string(value).ok()?;
    Some(encoded.trim_matches('"').to_string())
}

fn url_encode(value: &str) -> String {
    url_encode_with_escape_case(value, true)
}

fn url_encode_with_lowercase_escapes(value: &str) -> String {
    url_encode_with_escape_case(value, false)
}

fn url_encode_with_escape_case(value: &str, uppercase_hex: bool) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ if uppercase_hex => encoded.push_str(&format!("%{byte:02X}")),
            _ => encoded.push_str(&format!("%{byte:02x}")),
        }
    }
    encoded
}

fn json_slash_escaped(value: &str) -> String {
    value.replace('/', "\\/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_secret_removes_plaintext_json_escaped_and_url_encoded_git_token_forms() {
        let secret = "ghp_test/token+value";
        let input = r#"plain ghp_test/token+value json ghp_test\/token+value url https://ghp_test%2Ftoken%2Bvalue@github.com/org/repo.git"#;

        let redacted = redact_secret(input, secret);

        assert!(!redacted.contains("ghp_test/token+value"));
        assert!(!redacted.contains(r#"ghp_test\/token+value"#));
        assert!(!redacted.contains("ghp_test%2Ftoken%2Bvalue"));
        assert_eq!(redacted.matches("<redacted>").count(), 3);
    }

    #[test]
    fn redact_secret_removes_lowercase_url_encoded_forms() {
        let secret = "token/value+plus";
        let input = "https://token%2fvalue%2bplus@example.invalid";

        let redacted = redact_secret(input, secret);

        assert!(!redacted.contains("token%2fvalue%2bplus"));
        assert_eq!(redacted, "https://<redacted>@example.invalid");
    }

    #[test]
    fn redact_secret_preserves_case_while_matching_lowercase_url_escapes() {
        let secret = "GhP/Token+";
        let input = "https://GhP%2fToken%2b@example.invalid";

        let redacted = redact_secret(input, secret);

        assert!(!redacted.contains("GhP%2fToken%2b"));
        assert_eq!(redacted, "https://<redacted>@example.invalid");
    }

    #[test]
    fn redact_secret_ignores_empty_secret() {
        assert_eq!(redact_secret("abc", ""), "abc");
    }
}
