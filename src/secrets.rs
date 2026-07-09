use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use crate::redaction::redact_secret;
use crate::workspace::Workspace;
use std::collections::HashSet;
use std::path::Path;

pub const REDACTED: &str = "<redacted>";
pub const REDACT_ENV: &str = "OMAKURE_REDACT_SECRETS";
pub const REDACT_FILE_ENV: &str = "OMAKURE_REDACT_SECRETS_FILE";

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

    fn canonical(&self) -> String {
        format!("secret://{}/{}", self.provider, self.key)
    }
}

#[derive(Debug, Clone, Default)]
pub struct SecretAccess {
    scopes: HashSet<String>,
    allowed_refs: HashSet<String>,
    allow_all: bool,
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
        }
    }

    pub fn allow_all() -> Self {
        Self {
            allow_all: true,
            ..Self::default()
        }
    }

    fn can_use(&self, secret_ref: &SecretRef) -> Result<(), SecretResolveError> {
        if self.allow_all {
            return Ok(());
        }
        if !self.scopes.contains("secrets:use") {
            return Err(SecretResolveError::Denied(
                "secrets:use scope is required".to_string(),
            ));
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum SecretResolveError {
    Denied(String),
    NotFound,
    InvalidRef,
}

pub fn resolve_args(
    workspace: &Workspace,
    script_path: &Path,
    args: &[String],
    extra_env: &[(String, String)],
) -> Result<ResolvedArgs, (String, String)> {
    resolve_args_with_direct_secrets(workspace, script_path, args, extra_env, &[])
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
                .map_err(|err| (field.name.clone(), secret_error_message(err)))?
            {
                Some(value) => {
                    resolved = Some(ResolvedSecretValue {
                        value,
                        provider_ref: is_secret_ref(&candidate).then_some(candidate),
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

        if value.value.is_empty() {
            continue;
        }
        let persisted_value = value.provider_ref.as_deref().unwrap_or(REDACTED);
        if let Some(provider_ref) = &value.provider_ref {
            if !provider_refs
                .iter()
                .any(|existing| existing == provider_ref)
            {
                provider_refs.push(provider_ref.clone());
            }
        }
        set_existing_arg_value(&mut persisted_args, &flag, persisted_value);
        if !set_existing_arg_value(&mut execution_args, &flag, &value.value) {
            execution_args.push(flag.clone());
            execution_args.push(value.value.clone());
            persisted_args.push(flag);
            persisted_args.push(persisted_value.to_string());
        }
        if !secrets.iter().any(|secret| secret == &value.value) {
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

pub fn parse_direct_secrets(values: &[String]) -> Result<Vec<(String, String)>, String> {
    values
        .iter()
        .map(|value| {
            let Some((field, secret)) = value.split_once('=') else {
                return Err("invalid secret argument: expected FIELD=VALUE".to_string());
            };
            if field.trim().is_empty() {
                return Err("invalid secret: field name cannot be empty".to_string());
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
    if let Ok(path) = std::env::var(REDACT_FILE_ENV) {
        if let Ok(value) = std::fs::read_to_string(path) {
            if let Ok(secrets) = serde_json::from_str::<Vec<String>>(&value) {
                return secrets;
            }
        }
    }
    std::env::var(REDACT_ENV)
        .ok()
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
    if let Some(name) = value.strip_prefix("secret://env:") {
        let secret_ref = SecretRef {
            provider: "env".to_string(),
            key: name.to_string(),
        };
        return resolve_env_secret(&secret_ref, access);
    }
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

fn is_secret_ref(value: &str) -> bool {
    value.starts_with("secret://")
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

fn secret_error_message(err: SecretResolveError) -> String {
    match err {
        SecretResolveError::Denied(message) => message,
        SecretResolveError::NotFound => "secret ref not found".to_string(),
        SecretResolveError::InvalidRef => "invalid secret ref".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use std::fs;
    use tempfile::TempDir;

    fn script_with_secret(tmp: &TempDir) -> (Workspace, std::path::PathBuf) {
        let workspace = Workspace::new(tmp.path().to_path_buf());
        workspace.ensure_layout().unwrap();
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

        assert_eq!(err, "invalid secret argument: expected FIELD=VALUE");
        assert!(!err.contains("raw_secret_without_field"));
    }

    #[test]
    fn env_provider_resolves_allowed_ref() {
        let tmp = TempDir::new().unwrap();
        let (workspace, script) = script_with_secret(&tmp);
        std::env::set_var("OMAKURE_TEST_SECRET_REF", "from_process_env");

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
        std::env::remove_var("OMAKURE_TEST_SECRET_REF");
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
}
