//! The shipped node config templates, checked against the struct they seed.

/// The shipped config templates must not drift from the struct they seed.
///
/// There are three hand-maintained copies of the same document — the runtime
/// image, the Windows installer, and the transport e2e script — and a field
/// added to `NodeConfig` lands in none of them automatically. Because every new
/// field carries `#[serde(default)]`, a stale template still *parses*: the node
/// starts, the feature is silently off, and an operator reading the config it
/// was given has no way to learn the knob exists. That is exactly how
/// `remote_cue_scripts` went missing from the packaged image.
///
/// Comparing the parsed template to `NodeConfig::default()` catches it at the
/// commit that introduces it rather than in a container weeks later.
fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[test]
fn every_shipped_config_template_matches_the_default_it_seeds() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dockerfile = normalize_line_endings(
        &std::fs::read_to_string(root.join("Dockerfile")).expect("read Dockerfile"),
    );
    let installer = normalize_line_endings(
        &std::fs::read_to_string(root.join("src/installer.rs")).expect("read the installer"),
    );
    let script = normalize_line_endings(
        &std::fs::read_to_string(root.join("scripts/tasks/cert/direct-transport"))
            .expect("read the transport e2e script"),
    );
    let installer_sh = normalize_line_endings(
        &std::fs::read_to_string(root.join("scripts/install/install.sh"))
            .expect("read the shell installer"),
    );

    let expected = omakure::domain::NodeConfig::default();
    for (name, template) in [
        ("Dockerfile", extract_dockerfile_template(&dockerfile)),
        ("src/installer.rs", extract_installer_template(&installer)),
        // The production installer for Linux and macOS. It was outside this
        // test until a VM install failed: it wrote a `[trust]` block with no
        // `authorities` and no bootstrap hashes, so a machine installed by the
        // shipped installer could not be provisioned for signed-bundle
        // enrollment at all. The three templates that were covered had been
        // corrected; the one an operator actually runs had not.
        (
            "scripts/install/install.sh",
            extract_shell_installer_template(&installer_sh),
        ),
    ] {
        let parsed = omakure::domain::NodeConfig::parse(&template).unwrap_or_else(|error| {
            panic!("{name} template does not parse: {error:?}\n{template}")
        });
        assert_eq!(
            parsed, expected,
            "the {name} config template has drifted from NodeConfig::default() in a value"
        );
        // Values alone cannot catch this. Every field carries
        // `#[serde(default)]`, so an omitted key parses to the same value the
        // default has -- an equality check passes precisely when the operator
        // has no way to see the knob. The key *names* are the real assertion.
        for key in declared_keys(&expected) {
            assert!(
                template
                    .lines()
                    .any(|line| line.trim_start().starts_with(&key)),
                "the {name} config template never mentions `{key}`, so an operator \
                 reading the config they were given cannot discover it"
            );
        }
    }

    // The transport e2e script deliberately writes a *customised* config --
    // static peers, a direct bind, no discovery section -- so equality is the
    // wrong check, and demanding every key would only force noise like
    // `authorities = []` into a script that has no use for it. What it must
    // carry is the switches that decide whether remote execution is possible:
    // omit one and the node it starts takes the default silently, which is a
    // security posture nobody chose.
    let _ = &expected;
    for key in [
        "allow_remote_cues",
        "remote_cue_scripts",
        "remote_cue_batteries",
        "allow_baseline_push",
        "baseline_publishers",
    ] {
        assert!(
            script.contains(key),
            "scripts/tasks/cert/direct-transport is missing `{key}`, so the \
             node it starts takes the default for it without saying so"
        );
    }
}

/// Every key name the default config writes, read from the struct itself.
///
/// `direct_bind` is absent by construction -- it is skipped when `None` -- so
/// this asks for exactly the set a template is expected to spell out.
fn declared_keys(config: &omakure::domain::NodeConfig) -> Vec<String> {
    let rendered = toml::to_string(config).expect("the default config serializes");
    let keys: Vec<String> = rendered
        .lines()
        .filter(|line| !line.trim_start().starts_with('['))
        .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim().to_string()))
        .filter(|key| !key.is_empty())
        .collect();
    assert!(
        keys.len() >= 15,
        "the default config should spell out every switch; found {keys:?}"
    );
    keys
}

/// Recover the TOML from the Dockerfile's `printf '%s\n' 'line' \` block.
fn extract_dockerfile_template(dockerfile: &str) -> String {
    let start = dockerfile
        .find("printf '%s\\n' \\")
        .expect("the Dockerfile still writes node.toml with printf");
    let rest = &dockerfile[start..];
    let end = rest
        .find("> /etc/omakure/node.toml")
        .expect("the printf block still redirects to node.toml");
    rest[..end]
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim().trim_end_matches('\\').trim();
            let line = line.strip_prefix('\'')?;
            Some(line.strip_suffix('\'').unwrap_or(line).to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Recover the TOML from `node_config_text`'s quoted heredoc.
fn extract_shell_installer_template(installer: &str) -> String {
    let start = installer
        .find("node_config_text() {")
        .expect("the shell installer still emits node.toml from node_config_text");
    let rest = &installer[start..];
    let body = rest
        .find("<<'CONFIG'\n")
        .map(|at| &rest[at + "<<'CONFIG'\n".len()..])
        .expect("node_config_text still uses a quoted CONFIG heredoc");
    let end = body
        .find("\nCONFIG")
        .expect("the CONFIG heredoc is still terminated");
    body[..end].to_string()
}

/// Recover the TOML from the installer's single escaped string literal.
fn extract_installer_template(installer: &str) -> String {
    let start = installer
        .find("\"version = 1\\n\\n[node]")
        .expect("the installer still writes node.toml from a literal");
    let rest = &installer[start + 1..];
    let end = rest
        .find("discovery_secret_ref = \\\"\\\"\\n\"")
        .expect("the installer literal still ends at discovery_secret_ref");
    let body = &rest[..end + "discovery_secret_ref = \\\"\\\"\\n".len()];
    body.replace("\\n", "\n").replace("\\\"", "\"")
}
