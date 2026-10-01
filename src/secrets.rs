use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use crate::redaction::redact_secret;
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::path::Path;

pub const REDACTED: &str = "<redacted>";
pub const REDACT_FILE_ENV: &str = "OMAKURE_REDACT_SECRETS_FILE";

/// Scope that lets a run resolve secret values.
pub const SECRETS_USE_SCOPE: &str = "secrets:use";
/// Scope that lets Battery credentials resolve secret values.
pub const CREDENTIALS_USE_SCOPE: &str = "credentials:use";
/// Scope that lets a caller list secret metadata without resolving values.
pub const SECRETS_READ_METADATA_SCOPE: &str = "secrets:read-metadata";
/// Every scope that participates in a secret ACL.
pub const SECRET_SCOPES: [&str; 3] = [
    SECRETS_USE_SCOPE,
    CREDENTIALS_USE_SCOPE,
    SECRETS_READ_METADATA_SCOPE,
];

#[derive(Debug, Clone, Default)]
pub struct ResolvedArgs {
    pub execution_args: Vec<String>,
    pub persisted_args: Vec<String>,
    pub secrets: Vec<String>,
    pub provider_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedSecretValue {
    value: String,
    provider_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    pub provider: String,
    pub key: String,
}

impl SecretRef {
    pub fn parse(value: &str) -> Option<Self> {
        let rest = value.strip_prefix("secret://")?;
        let (provider, key) = rest.split_once('/')?;
        if provider.is_empty() || key.is_empty() || key.contains('/') || key.contains('\\') {
            return None;
        }
        Some(Self {
            provider: provider.to_string(),
            key: key.to_string(),
        })
    }

    pub fn canonical(&self) -> String {
        format!("secret://{}/{}", self.provider, self.key)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SecretAccess {
    scopes: HashSet<String>,
    allowed_refs: HashSet<String>,
    allow_all: bool,
    /// When set, `env` provider refs must still appear in `allowed_refs` even
    /// when `allow_all` is true. Closes arbitrary process-environment reads
    /// under an HTTP `--secret-ref '*'` wildcard (the wildcard grants every
    /// file/provider ref, but env vars must be enumerated explicitly).
    restrict_env_to_allowed_refs: bool,
}

impl SecretAccess {
    pub fn new<I, J, S, T>(scopes: I, allowed_refs: J) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: Into<String>,
        T: Into<String>,
    {
        Self {
            scopes: scopes.into_iter().map(Into::into).collect(),
            allowed_refs: allowed_refs.into_iter().map(Into::into).collect(),
            allow_all: false,
            restrict_env_to_allowed_refs: false,
        }
    }

    pub fn allow_all() -> Self {
        Self {
            allow_all: true,
            ..Self::default()
        }
    }

    /// Wildcard that grants every non-`env` ref but keeps `env` provider refs
    /// gated behind `env_refs`. Used for the HTTP operator `--secret-ref '*'`
    /// so a remote caller cannot read arbitrary process env vars.
    pub fn allow_all_non_env<I, J, S, T>(scopes: I, env_refs: J) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: Into<String>,
        T: Into<String>,
    {
        // Drop an `env` provider-wildcard: the whole point of the env gate is
        // that env vars are enumerated per exact key. A `secret://env/*` entry
        // would otherwise match every env ref through the provider-wildcard branch and re-grant blanket
        // process-env access, defeating the wildcard hardening.
        let allowed_refs = env_refs
            .into_iter()
            .map(Into::into)
            .filter(|r: &String| r != "secret://env/*")
            .collect();
        Self {
            scopes: scopes.into_iter().map(Into::into).collect(),
            allowed_refs,
            allow_all: true,
            restrict_env_to_allowed_refs: true,
        }
    }

    /// Whether `allow_all` must be bypassed for this ref because it names the
    /// `env` provider and env access is restricted to the explicit allow-list.
    fn env_gated(&self, secret_ref: &SecretRef) -> bool {
        self.restrict_env_to_allowed_refs && secret_ref.provider == "env"
    }

    fn can_use(&self, secret_ref: &SecretRef) -> Result<(), SecretResolveError> {
        if self.allow_all && !self.env_gated(secret_ref) {
            return Ok(());
        }
        if !self.has_use_scope() {
            return Err(SecretResolveError::Denied(
                "secrets:use or credentials:use scope is required".to_string(),
            ));
        }
        self.ref_allowed(secret_ref)
    }

    fn has_use_scope(&self) -> bool {
        self.scopes.contains(SECRETS_USE_SCOPE) || self.scopes.contains(CREDENTIALS_USE_SCOPE)
    }

    /// Metadata listing accepts `secrets:read-metadata` (or use scopes) + ref ACL.
    /// Never grants value resolution — callers must still use [`Self::can_use`].
    fn can_list_metadata(&self, secret_ref: &SecretRef) -> Result<(), SecretResolveError> {
        if self.allow_all && !self.env_gated(secret_ref) {
            return Ok(());
        }
        if !self.scopes.contains(SECRETS_READ_METADATA_SCOPE) && !self.has_use_scope() {
            return Err(SecretResolveError::Denied(
                "secrets:read-metadata scope is required".to_string(),
            ));
        }
        self.ref_allowed(secret_ref)
    }

    fn ref_allowed(&self, secret_ref: &SecretRef) -> Result<(), SecretResolveError> {
        let canonical = secret_ref.canonical();
        let provider_wildcard = format!("secret://{}/*", secret_ref.provider);
        if self.allowed_refs.contains(&canonical) || self.allowed_refs.contains(&provider_wildcard)
        {
            Ok(())
        } else {
            Err(SecretResolveError::Denied(
                "secret ref is not allowed".to_string(),
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretResolveError {
    #[error("{0}")]
    Denied(String),
    #[error("secret ref not found")]
    NotFound,
    #[error("invalid secret ref")]
    InvalidRef,
}

pub fn resolve_args_with_direct_secrets(
    workspace: &Workspace,
    script_path: &Path,
    args: &[String],
    extra_env: &[(String, String)],
    direct_secrets: &[(String, String)],
) -> Result<ResolvedArgs, (String, String)> {
    resolve_args_with_access(
        workspace,
        script_path,
        args,
        extra_env,
        direct_secrets,
        &SecretAccess::allow_all(),
    )
}

pub fn resolve_args_with_access(
    workspace: &Workspace,
    script_path: &Path,
    args: &[String],
    extra_env: &[(String, String)],
    direct_secrets: &[(String, String)],
    access: &SecretAccess,
) -> Result<ResolvedArgs, (String, String)> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let schema = match repo.read_schema(script_path) {
        Ok(schema) => schema,
        Err(_) => {
            return Ok(ResolvedArgs {
                execution_args: args.to_vec(),
                persisted_args: args.to_vec(),
                secrets: Vec::new(),
                provider_refs: Vec::new(),
            });
        }
    };

    let mut execution_args = args.to_vec();
    let mut persisted_args = args.to_vec();
    let mut secrets = Vec::new();
    let mut provider_refs = Vec::new();

    for field in schema.fields.iter().filter(|field| field.is_secret()) {
        let flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        let candidates = [
            direct_secret_value(direct_secrets, &field.name),
            find_arg_value(args, &flag),
            env_value(extra_env, &field.name),
            field.default.clone(),
        ];
        let mut resolved = None;
        for candidate in candidates.into_iter().flatten() {
            match resolve_secret_ref(workspace, &candidate, access)
                .map_err(|err| (field.name.clone(), err.to_string()))?
            {
                Some(value) => {
                    resolved = Some(ResolvedSecretValue {
                        value,
                        provider_ref: SecretRef::parse(&candidate)
                            .map(|secret_ref| secret_ref.canonical()),
                    });
                    break;
                }
                None => continue,
            }
        }

        let Some(value) = resolved else {
            if field.required.unwrap_or(false) {
                return Err((
                    field.name.clone(),
                    format!(
                        "expected `{}` on the command line or in the run environment",
                        flag
                    ),
                ));
            }
            continue;
        };

        let persisted_value = value.provider_ref.as_deref().unwrap_or(REDACTED);
        if let Some(provider_ref) = &value.provider_ref {
            if !provider_refs
                .iter()
                .any(|existing| existing == provider_ref)
            {
                provider_refs.push(provider_ref.clone());
            }
        }
        // Replace any existing occurrence of the flag (e.g. a literal
        // `secret://` ref passed on the command line) with the resolved value —
        // even when that value is empty, so the child never receives the raw
        // ref. Only append a brand-new flag when there is a non-empty value to
        // pass.
        let existed = set_existing_arg_value(&mut execution_args, &flag, &value.value);
        set_existing_arg_value(&mut persisted_args, &flag, persisted_value);
        if !existed && !value.value.is_empty() {
            execution_args.push(flag.clone());
            execution_args.push(value.value.clone());
            persisted_args.push(flag);
            persisted_args.push(persisted_value.to_string());
        }
        if !value.value.is_empty() && !secrets.iter().any(|secret| secret == &value.value) {
            secrets.push(value.value);
        }
    }

    Ok(ResolvedArgs {
        execution_args,
        persisted_args,
        secrets,
        provider_refs,
    })
}

pub fn validate_queued_secret_args_reconstructable(
    workspace: &Workspace,
    script_path: &Path,
    args: &[String],
) -> Result<(), (String, String)> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let schema = match repo.read_schema(script_path) {
        Ok(schema) => schema,
        Err(_) => return Ok(()),
    };
    for field in schema.fields.iter().filter(|field| field.is_secret()) {
        let flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        if let Some(value) = find_arg_value(args, &flag) {
            if !value.starts_with("secret://") {
                return Err((
                    field.name.clone(),
                    "queued secret args must use secret:// refs so workers can reconstruct them without persisted plaintext".to_string(),
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DirectSecretParseError {
    #[error("invalid secret argument: expected FIELD=VALUE")]
    MissingAssignment,
    #[error("invalid secret: field name cannot be empty")]
    EmptyField,
}

pub fn parse_direct_secrets(
    values: &[String],
) -> Result<Vec<(String, String)>, DirectSecretParseError> {
    values
        .iter()
        .map(|value| {
            let Some((field, secret)) = value.split_once('=') else {
                return Err(DirectSecretParseError::MissingAssignment);
            };
            if field.trim().is_empty() {
                return Err(DirectSecretParseError::EmptyField);
            }
            Ok((field.to_string(), secret.to_string()))
        })
        .collect()
}

pub fn redact_text(input: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .fold(input.to_string(), |acc, secret| redact_secret(&acc, secret))
}

pub fn secrets_env_value(secrets: &[String]) -> Option<String> {
    if secrets.is_empty() {
        None
    } else {
        serde_json::to_string(secrets).ok()
    }
}

pub fn secrets_from_env() -> Vec<String> {
    std::env::var(REDACT_FILE_ENV)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|value| serde_json::from_str::<Vec<String>>(&value).ok())
        .unwrap_or_default()
}

fn find_arg_value(args: &[String], flag: &str) -> Option<String> {
    for idx in 0..args.len() {
        let arg = &args[idx];
        if arg == flag {
            return args
                .get(idx + 1)
                .filter(|value| value.as_str() != REDACTED)
                .cloned();
        }
        if let Some(value) = arg.strip_prefix(&format!("{}=", flag)) {
            if value != REDACTED {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn set_existing_arg_value(args: &mut [String], flag: &str, value: &str) -> bool {
    let mut replaced = false;
    for idx in 0..args.len() {
        if args[idx] == flag {
            if let Some(existing) = args.get_mut(idx + 1) {
                *existing = value.to_string();
                replaced = true;
            }
        } else if args[idx].starts_with(&format!("{}=", flag)) {
            args[idx] = format!("{}={}", flag, value);
            replaced = true;
        }
    }
    replaced
}

fn env_value(extra_env: &[(String, String)], field_name: &str) -> Option<String> {
    extra_env
        .iter()
        .rev()
        .find(|(key, _)| key == field_name)
        .or_else(|| {
            let lower = field_name.to_ascii_lowercase();
            extra_env
                .iter()
                .rev()
                .find(|(key, _)| key.to_ascii_lowercase() == lower)
        })
        .map(|(_, value)| value.clone())
}

fn direct_secret_value(direct_secrets: &[(String, String)], field_name: &str) -> Option<String> {
    direct_secrets
        .iter()
        .rev()
        .find(|(key, _)| key == field_name)
        .or_else(|| {
            let lower = field_name.to_ascii_lowercase();
            direct_secrets
                .iter()
                .rev()
                .find(|(key, _)| key.to_ascii_lowercase() == lower)
        })
        .map(|(_, value)| value.clone())
}

fn resolve_secret_ref(
    workspace: &Workspace,
    value: &str,
    access: &SecretAccess,
) -> Result<Option<String>, SecretResolveError> {
    let Some(secret_ref) = SecretRef::parse(value) else {
        if value.starts_with("secret://") {
            return Err(SecretResolveError::InvalidRef);
        }
        return Ok(Some(value.to_string()));
    };
    if secret_ref.provider == "env" {
        return resolve_env_secret(&secret_ref, access);
    }
    access.can_use(&secret_ref)?;
    resolve_file_secret(workspace, &secret_ref)
}

/// Check whether `access` permits resolving `value` without fetching the secret.
pub fn check_secret_access(value: &str, access: &SecretAccess) -> Result<(), SecretResolveError> {
    let secret_ref = SecretRef::parse(value).ok_or(SecretResolveError::InvalidRef)?;
    access.can_use(&secret_ref)
}

/// Resolve a `secret://…` ref to its plaintext value under `access`.
/// Used by Battery HTTPS auth (GIT_ASKPASS) and other credential consumers.
pub fn resolve_secret_value(
    workspace: &Workspace,
    value: &str,
    access: &SecretAccess,
) -> Result<String, String> {
    match resolve_secret_ref(workspace, value, access) {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(SecretResolveError::NotFound.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

/// Metadata-only inventory of secrets visible under `access` (never values).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecretMetadata {
    pub id: String,
    pub source: String,
    pub delivery: String,
    pub allowed_targets: Vec<String>,
}

pub fn list_secret_metadata(workspace: &Workspace, access: &SecretAccess) -> Vec<SecretMetadata> {
    let mut out = Vec::new();
    // Env provider: only list refs explicitly allowed (never dump process env).
    for allowed in &access.allowed_refs {
        if let Some(secret_ref) = SecretRef::parse(allowed) {
            if secret_ref.provider == "env" && access.can_list_metadata(&secret_ref).is_ok() {
                out.push(SecretMetadata {
                    id: secret_ref.canonical(),
                    source: "env".to_string(),
                    delivery: "process-env".to_string(),
                    allowed_targets: vec!["run".to_string(), "battery".to_string()],
                });
            }
        }
    }
    // File providers: list keys from managed env files when provider wildcard or
    // exact refs are allowed. Values are never included.
    if let Ok(entries) = std::fs::read_dir(workspace.envs_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("conf") {
                continue;
            }
            if !valid_provider_name(stem) {
                continue;
            }
            let Ok(values) = crate::adapters::environments::read_managed_env_defaults(
                workspace.envs_dir(),
                stem,
            ) else {
                continue;
            };
            for key in values.keys() {
                let secret_ref = SecretRef {
                    provider: stem.to_string(),
                    key: key.clone(),
                };
                if access.can_list_metadata(&secret_ref).is_err() {
                    continue;
                }
                out.push(SecretMetadata {
                    id: secret_ref.canonical(),
                    source: format!("file:{stem}"),
                    delivery: "managed-env-file".to_string(),
                    allowed_targets: vec!["run".to_string(), "battery".to_string()],
                });
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

fn resolve_env_secret(
    secret_ref: &SecretRef,
    access: &SecretAccess,
) -> Result<Option<String>, SecretResolveError> {
    access.can_use(secret_ref)?;
    Ok(std::env::var(&secret_ref.key).ok())
}

fn resolve_file_secret(
    workspace: &Workspace,
    secret_ref: &SecretRef,
) -> Result<Option<String>, SecretResolveError> {
    if !valid_provider_name(&secret_ref.provider) {
        return Err(SecretResolveError::InvalidRef);
    }
    let values = crate::adapters::environments::read_managed_env_defaults(
        workspace.envs_dir(),
        &secret_ref.provider,
    )
    .map_err(|_| SecretResolveError::NotFound)?;
    Ok(values.get(&secret_ref.key.to_ascii_lowercase()).cloned())
}

fn valid_provider_name(name: &str) -> bool {
    !name.is_empty()
        && name != "active"
        && !name.starts_with('.')
        && !name.ends_with(".conf")
        && !name.contains("..")
        && !name.contains('/')
        && !name.contains('\\')
        && Path::new(name).components().count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::workspace_in;
    use crate::workspace::Workspace;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    const ISOLATED_ENV_TEST: &str = "OMAKURE_SECRETS_TEST_CHILD";
    const ISOLATED_ENV_MARKER: &str = "omakure_secrets_isolated_child_ran";

    fn run_with_isolated_env(key: &str, value: &str) -> bool {
        let current_thread = std::thread::current();
        let test_name = current_thread.name().expect("named test thread");
        if let Some(selected) = std::env::var_os(ISOLATED_ENV_TEST) {
            assert_eq!(selected.to_str(), Some(test_name));
            assert!(matches!(std::env::var(key), Ok(actual) if actual == value));
            println!("{ISOLATED_ENV_MARKER}");
            return true;
        }
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", test_name, "--nocapture"])
            .env(ISOLATED_ENV_TEST, test_name)
            .env(key, value)
            .output()
            .expect("run isolated secret test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !value.is_empty() {
            assert!(
                !stdout.contains(value),
                "isolated secret test leaked stdout"
            );
            assert!(
                !String::from_utf8_lossy(&output.stderr).contains(value),
                "isolated secret test leaked stderr"
            );
        }
        assert!(
            stdout.contains(ISOLATED_ENV_MARKER),
            "isolated secret test did not run"
        );
        assert!(output.status.success(), "isolated secret test failed");
        false
    }

    fn script_with_secret(tmp: &TempDir) -> (Workspace, std::path::PathBuf) {
        let workspace = workspace_in(tmp);
        let script = tmp.path().join("secret.sh");
        fs::write(
            &script,
            r#"#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Secret","Fields":[{"Name":"TOKEN","Type":"secret","Required":true,"Arg":"--token"}]}
# OMAKURE_SCHEMA_END
"#,
        )
        .unwrap();
        (workspace, script)
    }

    #[test]
    fn secret_ref_parser_accepts_generic_secret_uri() {
        let parsed = SecretRef::parse("secret://prod/token").unwrap();

        assert_eq!(parsed.provider, "prod");
        assert_eq!(parsed.key, "token");
    }

    #[test]
    fn parse_direct_secrets_does_not_echo_invalid_secret_value() {
        let err = parse_direct_secrets(&["raw_secret_without_field".into()]).unwrap_err();

        assert_eq!(err, DirectSecretParseError::MissingAssignment);
        assert_eq!(
            err.to_string(),
            "invalid secret argument: expected FIELD=VALUE"
        );
        assert!(!err.to_string().contains("raw_secret_without_field"));
    }

    #[test]
    fn parse_direct_secrets_rejects_empty_field_without_echoing_value() {
        let err = parse_direct_secrets(&[" =raw_secret_value".into()]).unwrap_err();

        assert_eq!(err, DirectSecretParseError::EmptyField);
        assert_eq!(
            err.to_string(),
            "invalid secret: field name cannot be empty"
        );
        assert!(!err.to_string().contains("raw_secret_value"));
    }

    #[test]
    fn check_secret_access_keeps_error_text_and_hides_ref() {
        let access = SecretAccess::new(["secrets:use"], ["secret://prod/allowed"]);
        let invalid = check_secret_access("secret://", &access).unwrap_err();
        assert_eq!(invalid, SecretResolveError::InvalidRef);
        assert_eq!(invalid.to_string(), "invalid secret ref");

        let denied = check_secret_access("secret://prod/raw_secret_value", &access).unwrap_err();
        assert_eq!(
            denied,
            SecretResolveError::Denied("secret ref is not allowed".into())
        );
        assert_eq!(denied.to_string(), "secret ref is not allowed");
        assert!(!denied.to_string().contains("raw_secret_value"));
    }

    #[test]
    fn env_provider_resolves_allowed_ref() {
        if !run_with_isolated_env("OMAKURE_TEST_SECRET_REF", "from_process_env") {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);

        let resolved = resolve_args_with_access(
            &workspace,
            &script,
            &[
                "--token".into(),
                "secret://env/OMAKURE_TEST_SECRET_REF".into(),
            ],
            &[],
            &[],
            &SecretAccess::allow_all(),
        )
        .unwrap();

        assert_eq!(resolved.execution_args, vec!["--token", "from_process_env"]);
        assert_eq!(
            resolved.persisted_args,
            vec!["--token", "secret://env/OMAKURE_TEST_SECRET_REF"]
        );
        assert_eq!(resolved.secrets, vec!["from_process_env"]);
    }

    #[test]
    fn empty_env_secret_replaces_literal_ref_in_execution_args() {
        if !run_with_isolated_env("OMAKURE_TEST_EMPTY_SECRET", "") {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);

        let resolved = resolve_args_with_access(
            &workspace,
            &script,
            &[
                "--token".into(),
                "secret://env/OMAKURE_TEST_EMPTY_SECRET".into(),
            ],
            &[],
            &[],
            &SecretAccess::allow_all(),
        )
        .unwrap();

        // Regression: an empty resolved value must REPLACE the literal ref, not
        // be skipped — previously the child received `--token secret://env/...`.
        assert_eq!(resolved.execution_args, vec!["--token", ""]);
        assert!(
            !resolved
                .execution_args
                .iter()
                .any(|arg| arg.starts_with("secret://")),
            "literal secret ref leaked into execution args: {:?}",
            resolved.execution_args
        );
        assert_eq!(
            resolved.persisted_args,
            vec!["--token", "secret://env/OMAKURE_TEST_EMPTY_SECRET"]
        );
        // An empty value must not enter the redaction list.
        assert!(resolved.secrets.is_empty());
    }

    #[test]
    fn env_wildcard_requires_explicit_env_ref_but_rides_provider_refs() {
        if !run_with_isolated_env("OMAKURE_TEST_F6_ENV", "env_value") {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);
        fs::write(workspace.envs_dir().join("prod.conf"), "TOKEN=file_value\n").unwrap();

        // Wildcard WITHOUT an explicit env ref: env reads are denied even though
        // non-env refs ride the wildcard (SSRF-exfil hardening).
        let no_env = SecretAccess::allow_all_non_env(["secrets:use"], Vec::<String>::new());
        let err = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://env/OMAKURE_TEST_F6_ENV".into()],
            &[],
            &[],
            &no_env,
        )
        .unwrap_err();
        assert_eq!(err.0, "TOKEN");
        assert!(err.1.contains("not allowed"));
        assert!(!err.1.contains("env_value"));

        // Same wildcard resolves a NON-env provider ref (file provider).
        let resolved = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://prod/token".into()],
            &[],
            &[],
            &no_env,
        )
        .unwrap();
        assert_eq!(resolved.execution_args, vec!["--token", "file_value"]);

        // Explicitly allow-listed env ref DOES resolve under the wildcard.
        let with_env =
            SecretAccess::allow_all_non_env(["secrets:use"], ["secret://env/OMAKURE_TEST_F6_ENV"]);
        let resolved_env = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://env/OMAKURE_TEST_F6_ENV".into()],
            &[],
            &[],
            &with_env,
        )
        .unwrap();
        assert_eq!(resolved_env.execution_args, vec!["--token", "env_value"]);
    }

    #[test]
    fn env_provider_wildcard_does_not_regrant_blanket_env_under_wildcard() {
        if !run_with_isolated_env("OMAKURE_TEST_A4_ENV", "leaked_value") {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);

        // Even with `secret://env/*` in the allow-list, the env gate must not
        // grant blanket env access under the wildcard.
        let access = SecretAccess::allow_all_non_env(["secrets:use"], ["secret://env/*"]);
        let err = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://env/OMAKURE_TEST_A4_ENV".into()],
            &[],
            &[],
            &access,
        )
        .unwrap_err();
        assert_eq!(err.0, "TOKEN");
        assert!(err.1.contains("not allowed"));
        assert!(!err.1.contains("leaked_value"));
    }

    #[test]
    fn file_provider_resolves_allowed_managed_env_ref() {
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);
        fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=from_file_provider\n",
        )
        .unwrap();

        let resolved = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://prod/token".into()],
            &[],
            &[],
            &SecretAccess::allow_all(),
        )
        .unwrap();

        assert_eq!(
            resolved.execution_args,
            vec!["--token", "from_file_provider"]
        );
        assert_eq!(
            resolved.persisted_args,
            vec!["--token", "secret://prod/token"]
        );
        assert_eq!(resolved.secrets, vec!["from_file_provider"]);
    }

    #[test]
    #[cfg(unix)]
    fn file_provider_rejects_symlinked_env_file() {
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);
        let outside = tmp.path().join("outside.conf");
        fs::write(&outside, "token=from_outside").unwrap();
        symlink(&outside, workspace.envs_dir().join("prod.conf")).unwrap();

        let err = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://prod/token".into()],
            &[],
            &[],
            &SecretAccess::allow_all(),
        )
        .unwrap_err();

        assert_eq!(err.0, "TOKEN");
        assert!(err.1.contains("secret ref not found"));
    }

    #[test]
    fn provider_resolution_requires_secrets_use_scope() {
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);
        fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=from_file_provider\n",
        )
        .unwrap();

        let err = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://prod/token".into()],
            &[],
            &[],
            &SecretAccess::new(Vec::<&str>::new(), ["secret://prod/token"]),
        )
        .unwrap_err();

        assert_eq!(err.0, "TOKEN");
        assert!(err.1.contains("secrets:use"));
        assert!(!err.1.contains("from_file_provider"));
    }

    #[test]
    fn provider_resolution_requires_acl_match() {
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);
        fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=from_file_provider\n",
        )
        .unwrap();

        let err = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://prod/token".into()],
            &[],
            &[],
            &SecretAccess::new(["secrets:use"], ["secret://prod/other"]),
        )
        .unwrap_err();

        assert_eq!(err.0, "TOKEN");
        assert!(err.1.contains("not allowed"));
        assert!(!err.1.contains("from_file_provider"));
    }

    #[test]
    fn missing_provider_ref_is_error_not_plaintext() {
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);

        let err = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://prod/missing".into()],
            &[],
            &[],
            &SecretAccess::allow_all(),
        )
        .unwrap_err();

        assert_eq!(err.0, "TOKEN");
        assert!(err.1.contains("secret ref not found"));
        assert!(!err.1.contains("secret://prod/missing"));
    }

    #[test]
    fn redaction_removes_provider_value_from_output_and_error_text() {
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);
        fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=from_file_provider\n",
        )
        .unwrap();

        let resolved = resolve_args_with_access(
            &workspace,
            &script,
            &["--token".into(), "secret://prod/token".into()],
            &[],
            &[],
            &SecretAccess::allow_all(),
        )
        .unwrap();

        assert_eq!(
            redact_text(
                "stdout from_file_provider stderr from_file_provider",
                &resolved.secrets
            ),
            "stdout <redacted> stderr <redacted>"
        );
        let persisted = resolved.persisted_args.join(" ");
        assert!(persisted.contains("secret://prod/token"));
        assert!(!persisted.contains("from_file_provider"));
    }

    #[test]
    fn list_secret_metadata_never_includes_values() {
        let tmp = TempDir::new().unwrap();
        let workspace = workspace_in(&tmp);
        fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=super-secret-token-value\nOTHER=also-secret\n",
        )
        .unwrap();

        let access = SecretAccess::new(
            ["secrets:read-metadata"],
            ["secret://prod/token", "secret://prod/other"],
        );
        let meta = list_secret_metadata(&workspace, &access);
        let serialized = serde_json::to_string(&meta).unwrap();

        assert!(meta.iter().any(|m| m.id == "secret://prod/token"));
        assert!(meta.iter().any(|m| m.id == "secret://prod/other"));
        assert!(meta.iter().all(|m| m.delivery == "managed-env-file"));
        assert!(!serialized.contains("super-secret-token-value"));
        assert!(!serialized.contains("also-secret"));
    }

    #[test]
    fn list_secret_metadata_respects_ref_acl_without_use_scope() {
        let tmp = TempDir::new().unwrap();
        let workspace = workspace_in(&tmp);
        fs::write(
            workspace.envs_dir().join("prod.conf"),
            "TOKEN=secret-a\nOTHER=secret-b\n",
        )
        .unwrap();

        let access = SecretAccess::new(["secrets:read-metadata"], ["secret://prod/token"]);
        let meta = list_secret_metadata(&workspace, &access);

        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].id, "secret://prod/token");
        assert!(!serde_json::to_string(&meta).unwrap().contains("secret-a"));
    }
}
