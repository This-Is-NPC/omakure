use super::*;
use pretty_assertions::assert_eq;

#[rstest]
#[case::exact_match(&["--target", "prod"], "--target", true)]
#[case::equals_syntax(&["--target=prod"], "--target", true)]
#[case::not_present(&["--other", "val"], "--target", false)]
#[case::empty_args(&[], "--target", false)]
fn test_cli_args_contain_flag(#[case] args: &[&str], #[case] flag: &str, #[case] expected: bool) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    assert_eq!(args_contain_flag(&args, flag), expected);
}

#[test]
fn test_resolve_script_path_exact_file() {
    let tmp = TempDir::new().unwrap();
    let script = tmp.path().join("deploy.sh");
    fs::write(&script, "#!/bin/bash").unwrap();

    let result = resolve_script_path("deploy.sh", tmp.path()).unwrap();
    assert_eq!(result, script.canonicalize().unwrap());
}

#[test]
fn test_resolve_script_path_extension_fallback() {
    let tmp = TempDir::new().unwrap();
    let script = tmp.path().join("deploy.sh");
    fs::write(&script, "#!/bin/bash").unwrap();

    let result = resolve_script_path("deploy", tmp.path()).unwrap();
    assert_eq!(result, script.canonicalize().unwrap());
}

#[test]
fn test_resolve_script_path_not_found() {
    let tmp = TempDir::new().unwrap();
    let result = resolve_script_path("nonexistent", tmp.path());
    assert!(result.is_err());
}

#[test]
fn test_resolve_script_path_absolute() {
    let tmp = TempDir::new().unwrap();
    let script = tmp.path().join("abs.sh");
    fs::write(&script, "#!/bin/bash").unwrap();

    let result = resolve_script_path(&script.to_string_lossy(), tmp.path()).unwrap();
    assert_eq!(result, script.canonicalize().unwrap());
}

#[test]
fn test_resolve_script_path_with_separator() {
    let tmp = TempDir::new().unwrap();
    let subdir = tmp.path().join("infra");
    fs::create_dir_all(&subdir).unwrap();
    let script = subdir.join("deploy.sh");
    fs::write(&script, "#!/bin/bash").unwrap();

    let result = resolve_script_path("infra/deploy.sh", tmp.path()).unwrap();
    assert_eq!(result, script.canonicalize().unwrap());
}

#[test]
fn test_resolve_script_path_directory_not_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("deploy.sh");
    fs::create_dir_all(&dir).unwrap();

    let result = resolve_script_path("deploy.sh", tmp.path());
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not a file"));
}

#[test]
fn test_check_required_fields_without_schema_is_permissive() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let script = tmp.path().join("plain.sh");
    write_file(&script, "#!/usr/bin/env bash\necho hi\n");

    assert!(check_required_fields(&ws, &script, &[]).is_ok());
}

#[test]
fn test_check_required_fields_accepts_default_and_override_flags() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let script = write_schema_script(
        &tmp,
        "deploy.sh",
        r#"{"Name":"Deploy","Fields":[{"Name":"target","Type":"string","Order":1,"Required":true},{"Name":"region","Type":"string","Order":2,"Required":true,"Arg":"--azure-region"},{"Name":"optional","Type":"string","Order":3,"Required":false}]}"#,
        "echo hi",
    );

    let args = vec![
        "--target=prod".to_string(),
        "--azure-region".to_string(),
        "eastus".to_string(),
    ];

    assert!(check_required_fields(&ws, &script, &args).is_ok());
}

#[test]
fn test_check_required_fields_returns_missing_field_and_message() {
    let tmp = TempDir::new().unwrap();
    let ws = workspace_in(&tmp);
    let script = write_schema_script(
        &tmp,
        "deploy.sh",
        r#"{"Name":"Deploy","Fields":[{"Name":"target","Type":"string","Order":1,"Required":true}]}"#,
        "echo hi",
    );

    let err = check_required_fields(&ws, &script, &[]).unwrap_err();

    assert_eq!(err.0, "target");
    assert!(err.1.contains("--target"));
}
