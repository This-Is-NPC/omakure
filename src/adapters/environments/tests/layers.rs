use super::super::layers::{merge_env_layers, parse_env_pairs_raw};
use super::super::values::expand_env_value;
use super::*;
use pretty_assertions::assert_eq;

// --- Case-preserving injectable parser + var expansion ---

#[test]
fn test_parse_env_pairs_raw_preserves_key_case() {
    let input = "PATH=/usr/bin\nVIRTUAL_ENV=/opt/venv\nMixed_Case=v";
    let result = parse_env_pairs_raw(input);
    assert_eq!(
        result,
        vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("VIRTUAL_ENV".to_string(), "/opt/venv".to_string()),
            ("Mixed_Case".to_string(), "v".to_string()),
        ]
    );
}

#[test]
fn test_parse_env_pairs_raw_line_handling_matches_defaults() {
    // export prefix, quotes, comments, empty-value skipping — but ordered
    // and case-preserved.
    let input = concat!(
        "export FOO=\"bar\"\n",
        "# comment\n",
        "; comment\n",
        "EMPTY=\n",
        "  SP  =  spaced value  \n",
        "SINGLE='q'\n",
    );
    let result = parse_env_pairs_raw(input);
    assert_eq!(
        result,
        vec![
            ("FOO".to_string(), "bar".to_string()),
            ("SP".to_string(), "spaced value".to_string()),
            ("SINGLE".to_string(), "q".to_string()),
        ]
    );
}

#[test]
fn test_merge_env_layers_expands_bare_and_braced_within_layer() {
    // BASE defined first; later values reference it in both forms.
    let input = concat!("BASE=/opt\n", "BARE=$BASE/bin\n", "BRACED=${BASE}/lib\n",);
    let result = merge_env_layers(&HashMap::new(), &[&parse_env_pairs_raw(input)]);
    assert_eq!(
        result,
        vec![
            ("BASE".to_string(), "/opt".to_string()),
            ("BARE".to_string(), "/opt/bin".to_string()),
            ("BRACED".to_string(), "/opt/lib".to_string()),
        ]
    );
}

// --- merge_env_layers: expansion sources the merged env incl. parent ---
// (regression coverage for task 1758)

#[test]
fn test_merge_env_layers_self_reference_prepends_to_base_path() {
    // `PATH=/x/bin:$PATH` must prepend to the base (parent) PATH, not
    // self-reference the file's own raw value. No doubled prefix, no
    // literal `$PATH` residue, system PATH preserved.
    let base = vars(&[("PATH", "/usr/bin:/bin")]);
    let layer = vec![("PATH".to_string(), "/x/bin:$PATH".to_string())];
    assert_eq!(
        merge_env_layers(&base, &[&layer]),
        vec![("PATH".to_string(), "/x/bin:/usr/bin:/bin".to_string())]
    );
}

#[test]
fn test_merge_env_layers_returns_only_layer_keys_not_base() {
    // The base (parent shell env) is an expansion SOURCE only; it must
    // never leak into the emitted pairs.
    let base = vars(&[("PATH", "/usr/bin"), ("SECRET_TOKEN", "shh")]);
    let layer = vec![("MY_VAR".to_string(), "hello".to_string())];
    assert_eq!(
        merge_env_layers(&base, &[&layer]),
        vec![("MY_VAR".to_string(), "hello".to_string())]
    );
}

#[test]
fn test_merge_env_layers_undefined_in_file_and_base_is_empty() {
    // A var absent from both the file layers AND the base expands empty.
    let base = vars(&[("PATH", "/usr/bin")]);
    let layer = vec![("X".to_string(), "a${MISSING}b".to_string())];
    assert_eq!(
        merge_env_layers(&base, &[&layer]),
        vec![("X".to_string(), "ab".to_string())]
    );
}

#[test]
fn test_merge_env_layers_later_layer_expands_against_earlier() {
    // The env-file layer (higher precedence) sees the active layer's
    // already-expanded value and overrides the key in place.
    let base = vars(&[("PATH", "/sys")]);
    let active = vec![("PATH".to_string(), "/active:$PATH".to_string())];
    let file = vec![("PATH".to_string(), "/file:$PATH".to_string())];
    assert_eq!(
        merge_env_layers(&base, &[&active, &file]),
        vec![("PATH".to_string(), "/file:/active:/sys".to_string())]
    );
}

#[test]
fn test_merge_env_layers_file_key_overrides_base_no_ref() {
    // A file key with no `$` reference simply overrides the base value and
    // is emitted verbatim (base value is not carried through).
    let base = vars(&[("HOST", "parent")]);
    let layer = vec![("HOST".to_string(), "fromfile".to_string())];
    assert_eq!(
        merge_env_layers(&base, &[&layer]),
        vec![("HOST".to_string(), "fromfile".to_string())]
    );
}

// --- resolve_active_env (layer 2 injector, spec section 1) ---

#[test]
fn test_resolve_active_env_none_when_no_active_pointer() {
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();
    fs::write(envs.join("dev.conf"), "HOST=localhost").unwrap();
    // No `active` pointer => nothing to inject.
    assert!(resolve_active_env(&envs).is_empty());
}

#[test]
fn test_resolve_active_env_reads_active_conf_case_preserving() {
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();
    fs::write(envs.join("dev.conf"), "PATH=/usr/bin\nMY_VAR=hello").unwrap();
    fs::write(envs.join("active"), "dev.conf\n").unwrap();

    // Keys are preserved verbatim (unlike the lowercasing prefill path)
    // and order is stable.
    assert_eq!(
        resolve_active_env(&envs),
        vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("MY_VAR".to_string(), "hello".to_string()),
        ]
    );
}

#[test]
fn test_resolve_active_env_missing_conf_is_best_effort_empty() {
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();
    fs::write(envs.join("active"), "ghost.conf\n").unwrap();
    // Unreadable/missing target must not fail the run — resolves empty.
    assert!(resolve_active_env(&envs).is_empty());
}

// --- resolve_run_env (layers 2 + 3 composition root, spec section 1) ---

#[test]
fn test_resolve_run_env_no_env_file_equals_active_env() {
    // With no --env-file, resolve_run_env is exactly the active env.
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();
    fs::write(envs.join("dev.conf"), "HOST=localhost\nPORT=8080").unwrap();
    fs::write(envs.join("active"), "dev.conf\n").unwrap();

    assert_eq!(
        resolve_run_env(&envs, None).unwrap(),
        resolve_active_env(&envs)
    );
}

#[test]
fn test_resolve_run_env_env_file_overrides_active_env() {
    // Layer 3 (--env-file) wins over layer 2 (active env) for the same
    // case-sensitive key; a key only in the env-file is appended; a key
    // only in the active env is preserved.
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();
    fs::write(envs.join("dev.conf"), "HOST=active\nONLY_ACTIVE=keep").unwrap();
    fs::write(envs.join("active"), "dev.conf\n").unwrap();

    let env_file = tmp.path().join("run.env");
    fs::write(&env_file, "HOST=fromfile\nONLY_FILE=added").unwrap();

    let merged = resolve_run_env(&envs, Some(&env_file)).unwrap();
    // HOST overridden in place; active-only preserved; file-only appended.
    assert_eq!(
        merged,
        vec![
            ("HOST".to_string(), "fromfile".to_string()),
            ("ONLY_ACTIVE".to_string(), "keep".to_string()),
            ("ONLY_FILE".to_string(), "added".to_string()),
        ]
    );
}

#[test]
fn test_resolve_run_env_env_file_only_no_active() {
    // No active env: the env-file pairs are the whole result.
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();

    let env_file = tmp.path().join("run.env");
    fs::write(&env_file, "TOKEN=abc").unwrap();

    assert_eq!(
        resolve_run_env(&envs, Some(&env_file)).unwrap(),
        vec![("TOKEN".to_string(), "abc".to_string())]
    );
}

#[test]
fn test_resolve_run_env_missing_env_file_is_error() {
    // An explicit --env-file path the user passed that does not exist
    // must be a hard error, not a silent skip.
    let tmp = TempDir::new().unwrap();
    let envs = tmp.path().join("envs");
    fs::create_dir_all(&envs).unwrap();

    let ghost = tmp.path().join("does-not-exist.env");
    let err = resolve_run_env(&envs, Some(&ghost)).unwrap_err();
    assert!(
        err.to_string().contains("does-not-exist.env"),
        "error should name the offending path, got: {}",
        err
    );
}

// --- expand_env_value grammar (spec section 2.6 worked examples) ---

fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[rstest]
#[case::bare("$FOO", "bar")]
#[case::braced("${FOO}", "bar")]
#[case::bare_suffix("$FOO/baz", "bar/baz")]
#[case::braced_suffix("${FOO}baz", "barbaz")]
#[case::undefined_bare("a$BAZ", "a")]
#[case::undefined_braced("x${BAZ}y", "xy")]
#[case::escaped_dollar("\\$FOO", "$FOO")]
#[case::literal_backslash_then_escaped_dollar("\\\\$FOO", "\\$FOO")]
#[case::command_sub_literal("$(echo hi)", "$(echo hi)")]
#[case::backtick_literal("`date`", "`date`")]
#[case::rich_form_empty("${FOO:-x}", "")]
#[case::digit_literal("$1abc", "$1abc")]
#[case::unterminated_brace("${FOO", "${FOO")]
#[case::bare_no_name("$ ", "$ ")]
#[case::backslash_literal("a\\b", "a\\b")]
#[case::adjacent_forms("$FOO${FOO}", "barbar")]
#[case::escaped_then_expanded("\\$FOO$FOO", "$FOObar")]
#[case::unterminated_after_expansion("$FOO${BAZ", "bar${BAZ")]
#[case::invalid_empty_braces("a${}b", "ab")]
#[case::unicode_surrounding_reference("λ$FOO🙂", "λbar🙂")]
fn test_expand_env_value_grammar(#[case] input: &str, #[case] expected: &str) {
    let env = vars(&[("FOO", "bar"), ("OMAKURE_RUN_ID", "r-1")]);
    assert_eq!(expand_env_value(input, &env), expected);
}

#[test]
fn test_expand_env_value_no_recursion() {
    // FOO expands to a literal that itself looks like a reference; the
    // output must NOT be re-scanned.
    let env = vars(&[("FOO", "$BAR"), ("BAR", "deep")]);
    assert_eq!(expand_env_value("$FOO", &env), "$BAR");
}

#[test]
fn test_merge_env_layers_command_substitution_not_executed() {
    let input = "CMD=$(rm -rf /)\nTICK=`date`";
    let result = merge_env_layers(&HashMap::new(), &[&parse_env_pairs_raw(input)]);
    assert_eq!(
        result,
        vec![
            ("CMD".to_string(), "$(rm -rf /)".to_string()),
            ("TICK".to_string(), "`date`".to_string()),
        ]
    );
}
