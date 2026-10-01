use super::*;
#[cfg(unix)]
use crate::operations::battery::git::{run_git_capture_with_context, run_git_with_context};

#[cfg(unix)]
#[test]
fn git_execution_preserves_spawn_and_exit_errors() {
    let context = GitExecContext {
        policy: GitTransportPolicy::Default,
        askpass: None,
        http_pin: None,
        global_config: None,
    };
    let missing = GitCommandSpec {
        program: "/nonexistent/omakure-git".into(),
        args: vec![],
    };
    let spawn = run_git_with_context(missing, &context).unwrap_err();
    assert_eq!(spawn.code, OperationErrorCode::GitFailed);
    assert!(spawn.message.starts_with("failed to spawn git: "));

    let dir = crate::util::exec::generated_executable_tempdir().unwrap();
    let shim = dir.path().join("git-failure");
    crate::util::exec::write_generated_executable(
        &shim,
        b"#!/bin/sh\nprintf 'git failed\\n' >&2\nexit 7\n",
    )
    .unwrap();
    let failed = GitCommandSpec {
        program: shim.to_string_lossy().into_owned(),
        args: vec![],
    };
    let run = run_git_with_context(failed.clone(), &context).unwrap_err();
    assert_eq!(run.code, OperationErrorCode::GitFailed);
    assert_eq!(run.message, "git failed");
    let capture = run_git_capture_with_context(failed, &context).unwrap_err();
    assert_eq!(capture.code, OperationErrorCode::GitFailed);
    assert_eq!(capture.message, "git failed");
}

fn file_backed_askpass_auth(workspace: &Workspace, token: &str) -> BatteryAuth {
    fs::write(
        workspace.envs_dir().join("askpass.conf"),
        format!("TOKEN={token}\n"),
    )
    .unwrap();
    BatteryAuth {
        method: BatteryAuthMethod::HttpsTokenRef,
        token_ref: "secret://askpass/token".into(),
    }
}

#[test]
fn git_http_pinning_probe_retries_after_transient_failure() {
    let cache = std::sync::Mutex::new(None);
    let attempts = Cell::new(0);
    let err = assert_git_http_pinning_supported_with(&cache, || {
        attempts.set(attempts.get() + 1);
        Err(OperationError::new(
            OperationErrorCode::GitFailed,
            "temporary spawn failure",
        ))
    })
    .unwrap_err();

    assert_eq!(err.code, OperationErrorCode::GitFailed);
    assert!(cache.lock().unwrap().is_none());

    assert_git_http_pinning_supported_with(&cache, || {
        attempts.set(attempts.get() + 1);
        Ok("http.curloptResolve\n".into())
    })
    .unwrap();
    assert_eq!(attempts.get(), 2);

    assert_git_http_pinning_supported_with(&cache, || {
        panic!("conclusive probe result should be cached")
    })
    .unwrap();
}

#[test]
fn git_http_pinning_probe_caches_definitive_unsupported_result() {
    let cache = std::sync::Mutex::new(None);
    let err =
        assert_git_http_pinning_supported_with(&cache, || Ok("other.key\n".into())).unwrap_err();

    assert_eq!(err.code, OperationErrorCode::GitFailed);
    assert_eq!(*cache.lock().unwrap(), Some(false));
    assert_git_http_pinning_supported_with(&cache, || {
        panic!("conclusive probe result should be cached")
    })
    .unwrap_err();
}

#[test]
fn git_http_pinning_probe_single_flights_concurrent_callers() {
    let cache = std::sync::Arc::new(std::sync::Mutex::new(None));
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let start_barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let cache = std::sync::Arc::clone(&cache);
            let attempts = std::sync::Arc::clone(&attempts);
            let start_barrier = std::sync::Arc::clone(&start_barrier);
            std::thread::spawn(move || {
                start_barrier.wait();
                assert_git_http_pinning_supported_with(&cache, || {
                    attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    Ok("http.curloptResolve\n".into())
                })
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap().unwrap();
    }

    assert_eq!(
        attempts.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "concurrent callers must single-flight onto one probe spawn"
    );
}

#[test]
fn git_http_pinning_probe_single_flights_through_repeated_transient_failures() {
    let cache = std::sync::Arc::new(std::sync::Mutex::new(None));
    let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let start_barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let cache = std::sync::Arc::clone(&cache);
            let attempts = std::sync::Arc::clone(&attempts);
            let in_flight = std::sync::Arc::clone(&in_flight);
            let max_in_flight = std::sync::Arc::clone(&max_in_flight);
            let start_barrier = std::sync::Arc::clone(&start_barrier);
            std::thread::spawn(move || {
                start_barrier.wait();
                loop {
                    let result = assert_git_http_pinning_supported_with(&cache, || {
                        let concurrent =
                            in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        max_in_flight.fetch_max(concurrent, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        if attempt < 5 {
                            Err(OperationError::new(
                                OperationErrorCode::GitFailed,
                                "temporary spawn failure",
                            ))
                        } else {
                            Ok("http.curloptResolve\n".into())
                        }
                    });
                    if result.is_ok() {
                        return;
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(
        max_in_flight.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "at most one probe may run at a time, even while callers retry after transient failures"
    );
    assert!(
        attempts.load(std::sync::atomic::Ordering::SeqCst) >= 6,
        "expected 5 transient failures followed by 1 successful probe"
    );
}

#[test]
fn git_command_removes_api_token() {
    let command = git_command(
        &GitCommandSpec {
            program: "git".into(),
            args: vec!["status".into()],
        },
        GitTransportPolicy::Default,
    );

    assert!(command
        .get_envs()
        .any(|(k, v)| k == "OMAKURE_API_TOKEN" && v.is_none()));
}

#[test]
fn git_config_preparation_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let path = prepare_git_config_in(dir.path()).unwrap();
    assert_eq!(path, dir.path().join("git-empty-config"));
    assert!(fs::read(&path).unwrap().is_empty());

    fs::write(&path, b"stale config").unwrap();
    let repeated_path = prepare_git_config_in(dir.path()).unwrap();
    assert_eq!(repeated_path, path);
    assert!(fs::read(&path).unwrap().is_empty());
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn git_command_disables_prompts_and_external_config() {
    let spec = GitCommandSpec {
        program: "git".into(),
        args: vec!["status".into()],
    };
    let command = git_command(&spec, GitTransportPolicy::Default);
    let envs: Vec<_> = command.get_envs().collect();

    assert!(envs.iter().any(|(key, value)| {
        *key == "GIT_TERMINAL_PROMPT" && value.map(|v| v == "0").unwrap_or(false)
    }));
    assert!(envs.iter().any(|(key, value)| {
        *key == "GIT_ALLOW_PROTOCOL" && value.map(|v| v == "file:https:http").unwrap_or(false)
    }));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_ASKPASS" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "SSH_ASKPASS" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_SSH" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_SSH_COMMAND" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_TEMPLATE_DIR" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_EXEC_PATH" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "HOME" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "XDG_CONFIG_HOME" && value.is_none()));
    assert!(envs.iter().any(|(key, value)| {
        *key == "GIT_CONFIG_NOSYSTEM" && value.map(|v| v == "1").unwrap_or(false)
    }));
    assert!(envs.iter().any(|(key, value)| {
        *key == "GIT_ALLOW_PROTOCOL" && value.map(|v| v == "file:https:http").unwrap_or(false)
    }));
    assert!(envs.iter().any(|(key, value)| {
        *key == "GIT_CONFIG_GLOBAL"
            && value
                .map(|v| v.to_string_lossy().ends_with("git-empty-config"))
                .unwrap_or(false)
    }));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_CONFIG_SYSTEM" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_CONFIG_COUNT" && value.is_none()));
    assert!(envs
        .iter()
        .any(|(key, value)| *key == "GIT_CONFIG_PARAMETERS" && value.is_none()));
}

#[test]
fn git_command_can_restrict_protocols_to_https() {
    let spec = GitCommandSpec {
        program: "git".into(),
        args: vec!["status".into()],
    };
    let command = git_command(&spec, GitTransportPolicy::HttpsOnly);
    let envs: Vec<_> = command.get_envs().collect();

    assert!(envs.iter().any(|(key, value)| {
        *key == "GIT_ALLOW_PROTOCOL" && value.map(|v| v == "https").unwrap_or(false)
    }));
}

#[test]
fn git_http_command_disables_redirects_proxies_and_pins_verified_host() {
    let spec = GitCommandSpec {
        program: "git".into(),
        args: vec!["fetch".into()],
    };
    let pin = GitHttpPin {
        host: "git.example.test".into(),
        port: 8443,
        address: "203.0.113.10".parse().unwrap(),
        credential_authority: "git.example.test:8443".into(),
    };
    let command = git_command_with_context(
        &spec,
        &GitExecContext {
            policy: GitTransportPolicy::HttpsOnly,
            askpass: None,
            http_pin: Some(&pin),
            global_config: Some(Path::new(".omakure/git-empty-config")),
        },
    );
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    assert!(args
        .windows(2)
        .any(|pair| pair == ["-c", "http.followRedirects=false"]));
    assert!(args.windows(2).any(|pair| pair == ["-c", "http.proxy="]));
    assert!(args.windows(2).any(|pair| {
        pair == [
            "-c",
            "http.curloptResolve=git.example.test:8443:203.0.113.10",
        ]
    }));
    for proxy in [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ] {
        assert!(command
            .get_envs()
            .any(|(key, value)| env_key_eq(key, proxy) && value.is_none()));
    }
}

#[test]
fn git_http_pin_formats_ipv6_for_curl_and_uses_url_port() {
    let pin = resolve_public_git_endpoint("https://[2606:4700:4700::1111]:9443/repository.git")
        .unwrap()
        .unwrap();

    assert_eq!(pin.host, "2606:4700:4700::1111");
    assert_eq!(pin.port, 9443);
    assert_eq!(pin.credential_authority(), "[2606:4700:4700::1111]:9443");
    assert_eq!(
        pin.curlopt_resolve(),
        "[2606:4700:4700::1111]:9443:[2606:4700:4700::1111]"
    );
}

#[test]
fn git_specs_disable_hooks_submodules_and_checkout_detached() {
    let cache = Path::new("/tmp/cache/azure");
    let clone = git_clone_spec("https://example.invalid/azure.git", cache);
    assert_eq!(clone.program, "git");
    assert!(clone.args.contains(&"core.hooksPath=/dev/null".to_string()));
    assert!(clone.args.contains(&"protocol.ext.allow=never".to_string()));
    assert!(clone.args.contains(&"--no-recurse-submodules".to_string()));

    let command = git_command(&clone, GitTransportPolicy::Default);
    let config_args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(config_args
        .windows(2)
        .any(|pair| { pair[0] == "-c" && pair[1] == "core.autocrlf=false" }));
    #[cfg(windows)]
    assert!(config_args
        .windows(2)
        .any(|pair| { pair[0] == "-c" && pair[1] == "core.filemode=false" }));

    let fetch = git_fetch_spec(cache, "main");
    assert!(fetch.args.contains(&"--no-recurse-submodules".to_string()));

    let checkout = git_checkout_detached_spec(cache, "0123456789abcdef");
    assert!(checkout.args.contains(&"--detach".to_string()));
}

#[test]
fn local_git_config_rejects_every_blocked_section_and_key_with_exact_error() {
    for (config, offending_entry) in [
        ("[include]\npath = /tmp/other", "include"),
        (
            "[includeIf \"gitdir:/tmp\"]\npath = /tmp/other",
            "includeif \"gitdir:/tmp\"",
        ),
        ("[includeIf.extra]\npath = /tmp/other", "includeif.extra"),
        ("[http]\n\tfollowRedirects = true", "http"),
        (
            "[http \"https://example.test\"]\n\tcurloptResolve = example.test:443:127.0.0.1",
            "http \"https://example.test\"",
        ),
        ("[credential]\nHelper = value", "credential.helper"),
        (
            "[credential \"https://example.test\"]\nhelper = value",
            "credential \"https://example.test\".helper",
        ),
        ("[core]\naskPass = value", "core.askpass"),
        ("[core]\nsshCommand = value", "core.sshcommand"),
        ("[core]\nworktree = value", "core.worktree"),
        (
            "[extensions]\nworktreeConfig = true",
            "extensions.worktreeconfig",
        ),
        (
            "[url \"https://example.test\"]\ninsteadOf = value",
            "url \"https://example.test\".insteadof",
        ),
        (
            "[remote \"origin\"]\n\tproxy = http://127.0.0.1:8080",
            "remote \"origin\".proxy",
        ),
        (
            "[remote \"origin\"]\nproxyAuthMethod = value",
            "remote \"origin\".proxyauthmethod",
        ),
    ] {
        let error = reject_unsafe_git_config_text(config).unwrap_err();
        assert_eq!(error.code, OperationErrorCode::Conflict, "{config}");
        assert_eq!(
            error.message,
            format!("battery cache has unsafe local git config: {offending_entry}"),
            "{config}"
        );
    }
}

#[test]
fn local_git_config_accepts_near_misses_and_ignores_comments() {
    let config = "# [include]\n; [http]\n[includeExtra]\npath = /tmp/other\n[includeifExtra]\npath = /tmp/other\n[httpExtra]\nproxy = value\n[credential]\nuseHttpPath = true\n[core]\nhooksPath = /dev/null\n[extensions]\nobjectFormat = sha1\n[url \"https://example.test\"]\npushInsteadOf = value\n[remote \"origin\"]\nurl = https://example.test/repo.git\n";
    assert_eq!(reject_unsafe_git_config_text(config), Ok(()));
}

#[test]
fn local_git_config_reports_first_unsafe_entry() {
    let config = "[core]\n  AskPass = helper\n[include]\npath = /tmp/other\n";
    let error = reject_unsafe_git_config_text(config).unwrap_err();
    assert_eq!(error.code, OperationErrorCode::Conflict);
    assert_eq!(
        error.message,
        "battery cache has unsafe local git config: core.askpass"
    );
}

#[test]
fn git_stderr_redacts_credentials() {
    let msg = sanitize_git_stderr(
        "fatal: unable to access 'https://user:secret@example.invalid/repo.git'",
    );
    assert!(!msg.contains("secret"));
    assert!(msg.contains("<redacted>"));
}

#[test]
fn prepare_git_askpass_writes_0600_files_and_redacts_token() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    let plaintext = "askpass-redact-me-token-xyz";
    let auth = file_backed_askpass_auth(&ws, plaintext);
    let guard = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
        .unwrap()
        .expect("askpass");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&guard.script_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
        let token_mode = fs::metadata(guard.script_path.parent().unwrap().join("token"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(token_mode, 0o600);
    }
    let redacted = sanitize_git_output(
        &format!("fatal: Authentication failed for token {plaintext}"),
        Some(plaintext),
    );
    assert!(!redacted.contains(plaintext));
    assert!(redacted.contains("<redacted>"));
    let script = fs::read_to_string(&guard.script_path).unwrap();
    assert!(script.contains("\"$DIR/token\""));
    assert!(!script.contains(plaintext));
    assert!(!script.contains(
        guard
            .script_path
            .parent()
            .unwrap()
            .to_string_lossy()
            .as_ref()
    ));
    drop(guard);
}

#[test]
fn prepare_git_askpass_uses_distinct_directories_per_call() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    let auth = file_backed_askpass_auth(&ws, "tok-a");
    let a = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
        .unwrap()
        .unwrap();
    let b = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
        .unwrap()
        .unwrap();
    assert_ne!(a.script_path.parent(), b.script_path.parent());
    assert!(a.script_path.parent().unwrap().exists());
    assert!(b.script_path.parent().unwrap().exists());
}

#[test]
fn git_command_with_askpass_sets_git_askpass_env() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    let auth = file_backed_askpass_auth(&ws, "tok");
    let guard = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
        .unwrap()
        .unwrap();
    let pin = GitHttpPin {
        host: "git.example.test".into(),
        port: 443,
        address: "203.0.113.10".parse().unwrap(),
        credential_authority: "git.example.test".into(),
    };
    let command = git_command_with_context(
        &GitCommandSpec {
            program: "git".into(),
            args: vec!["status".into()],
        },
        &GitExecContext {
            policy: GitTransportPolicy::HttpsOnly,
            askpass: Some(&guard),
            http_pin: Some(&pin),
            global_config: Some(Path::new(".omakure/git-empty-config")),
        },
    );
    let envs: Vec<_> = command.get_envs().collect();
    assert!(envs.iter().any(|(k, v)| {
        *k == "GIT_ASKPASS"
            && v.map(|p| p == guard.script_path.as_os_str())
                .unwrap_or(false)
    }));
    assert!(envs
        .iter()
        .any(|(k, v)| { *k == "GIT_TERMINAL_PROMPT" && v.map(|v| v == "0").unwrap_or(false) }));
    assert!(envs.iter().any(|(k, v)| {
        *k == "OMAKURE_GIT_AUTHORITY" && v.map(|v| v == "git.example.test").unwrap_or(false)
    }));
    drop(guard);
}

#[cfg(unix)]
#[test]
fn git_askpass_refuses_credentials_for_another_host() {
    let dir = TempDir::new().unwrap();
    let ws = workspace_in(&dir);
    let auth = file_backed_askpass_auth(&ws, "host-bound-token");
    let guard = prepare_git_askpass(&ws, Some(&auth), &SecretAccess::allow_all())
        .unwrap()
        .unwrap();

    let run_askpass = |prompt: &str| {
        Command::new(&guard.script_path)
            .arg(prompt)
            .env("OMAKURE_GIT_AUTHORITY", "git.example.test")
            .output()
            .unwrap_or_else(|err| panic!("failed to execute askpass script: {err}"))
    };

    let allowed = run_askpass("Password for 'https://x-access-token@git.example.test':");
    let denied = run_askpass("Password for 'https://x-access-token@internal.example':");
    let suffix_denied = run_askpass("Password for 'https://x-access-token@git.example.test.evil':");

    assert!(allowed.status.success());
    assert_eq!(
        String::from_utf8_lossy(&allowed.stdout).trim(),
        "host-bound-token"
    );
    assert!(!denied.status.success());
    assert!(denied.stdout.is_empty());
    assert!(!suffix_denied.status.success());
    assert!(suffix_denied.stdout.is_empty());
    drop(guard);
}
