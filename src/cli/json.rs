//! JSON envelope helper used by every AI-facing CLI verb.
//!
//! All `--json` output in the binary flows through this module so the
//! envelope shape and `schema_version` constant live in exactly one place.
//! Agents can branch on `ok` and never need to parse human text.
//!
//! Envelope shape:
//!
//! ```json
//! {
//!   "ok": true,
//!   "data": <payload>,
//!   "error": null,
//!   "schema_version": "1"
//! }
//! ```
//!
//! On failure `ok` is false, `data` is null, and `error` is
//! `{ "code": "<stable-string>", "message": "<human message>" }`.

use serde::Serialize;
use serde_json::{json, Value};

/// Stable schema version for the AI JSON envelope. Bumped when the envelope
/// or any documented `data` shape changes in a non-backward-compatible way.
pub const SCHEMA_VERSION: &str = "1";

/// Stable error codes returned in `error.code`. Strings are stable parts of
/// the AI contract; renaming any of them is a breaking change.
pub mod codes {
    pub const NOT_FOUND: &str = "not_found";
    pub const SCHEMA_INVALID: &str = "schema_invalid";
    pub const SCRIPT_EXISTS: &str = "script_exists";
    pub const MISSING_REQUIRED_FIELD: &str = "missing_required_field";
    pub const INVALID_ARGUMENT: &str = "invalid_argument";
    pub const NOT_IMPLEMENTED: &str = "not_implemented";
    pub const INTERNAL: &str = "internal";
}

/// Build an `ok: true` envelope around a serializable payload.
pub fn ok_envelope<T: Serialize>(data: T) -> Value {
    json!({
        "ok": true,
        "data": data,
        "error": null,
        "schema_version": SCHEMA_VERSION,
    })
}

/// Build an `ok: false` envelope with a stable error code and a human
/// message. The code must come from [`codes`].
pub fn err_envelope(code: &str, message: impl Into<String>) -> Value {
    json!({
        "ok": false,
        "data": null,
        "error": {
            "code": code,
            "message": message.into(),
        },
        "schema_version": SCHEMA_VERSION,
    })
}

/// Print a successful envelope to stdout, followed by a newline.
pub fn print_ok<T: Serialize>(data: T) {
    let envelope = ok_envelope(data);
    println!("{}", envelope);
}

/// Print an error envelope to stdout (not stderr — agents read stdout) and
/// return so the caller can also set a non-zero exit code via the usual
/// error propagation path.
pub fn print_err(code: &str, message: impl Into<String>) {
    let envelope = err_envelope(code, message);
    println!("{}", envelope);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_is_one() {
        assert_eq!(SCHEMA_VERSION, "1");
    }

    #[test]
    fn ok_envelope_shape() {
        let env = ok_envelope(json!({"hello": "world"}));
        assert_eq!(env["ok"], true);
        assert_eq!(env["data"]["hello"], "world");
        assert!(env["error"].is_null());
        assert_eq!(env["schema_version"], "1");
    }

    #[test]
    fn err_envelope_shape() {
        let env = err_envelope(codes::NOT_FOUND, "missing thing");
        assert_eq!(env["ok"], false);
        assert!(env["data"].is_null());
        assert_eq!(env["error"]["code"], "not_found");
        assert_eq!(env["error"]["message"], "missing thing");
        assert_eq!(env["schema_version"], "1");
    }

    #[test]
    fn err_envelope_code_strings_are_stable() {
        // These strings are part of the public AI contract. Renaming any of
        // them is a breaking change — this test exists to make sure such a
        // rename triggers a deliberate test update, not a silent slip.
        assert_eq!(codes::NOT_FOUND, "not_found");
        assert_eq!(codes::SCHEMA_INVALID, "schema_invalid");
        assert_eq!(codes::SCRIPT_EXISTS, "script_exists");
        assert_eq!(codes::MISSING_REQUIRED_FIELD, "missing_required_field");
        assert_eq!(codes::INVALID_ARGUMENT, "invalid_argument");
        assert_eq!(codes::NOT_IMPLEMENTED, "not_implemented");
        assert_eq!(codes::INTERNAL, "internal");
    }

    #[test]
    fn ok_envelope_array_data() {
        let env = ok_envelope(vec![1, 2, 3]);
        assert!(env["data"].is_array());
        assert_eq!(env["data"][0], 1);
        assert_eq!(env["data"][2], 3);
    }
}
