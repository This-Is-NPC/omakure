use super::*;

#[test]
fn test_parse_api_defaults() {
    let cli = parse(&["api"]).unwrap();
    match cli.command.unwrap() {
        Commands::Api(args) => {
            assert_eq!(args.bind.to_string(), "127.0.0.1:7878");
            assert!(!args.allow_non_loopback);
            assert!(args.tokens_file.is_none());
        }
        _ => panic!("expected Api"),
    }
}

#[test]
fn test_parse_token_generate() {
    let cli = parse(&[
        "token",
        "generate",
        "--id",
        "ci",
        "--scope",
        "runs:read",
        "--scope",
        "scripts:read",
        "--append",
        "/tmp/tokens.toml",
        "--confirmed",
    ])
    .unwrap();
    match cli.command.unwrap() {
        Commands::Token(args) => match args.command {
            TokenCommand::Generate(g) => {
                assert_eq!(g.id, "ci");
                assert_eq!(g.scopes, vec!["runs:read", "scripts:read"]);
                assert_eq!(
                    g.append.as_deref(),
                    Some(std::path::Path::new("/tmp/tokens.toml"))
                );
                assert!(g.confirmed);
            }
        },
        _ => panic!("expected Token"),
    }
}

#[test]
fn test_parse_api_bind_flags() {
    let cli = parse(&["api", "--bind", "0.0.0.0:7878", "--allow-non-loopback"]).unwrap();
    match cli.command.unwrap() {
        Commands::Api(args) => {
            assert_eq!(args.bind.to_string(), "0.0.0.0:7878");
            assert!(args.allow_non_loopback);
        }
        _ => panic!("expected Api"),
    }
}

#[test]
fn test_parse_api_policy_flag() {
    let cli = parse(&["api", "--policy", "/etc/omakure/policy.toml"]).unwrap();
    match cli.command.unwrap() {
        Commands::Api(args) => {
            assert_eq!(
                args.policy.as_deref(),
                Some(std::path::Path::new("/etc/omakure/policy.toml"))
            );
        }
        _ => panic!("expected Api"),
    }
}

#[test]
fn test_api_help_surface_exists() {
    let command = Cli::command();
    let api = command
        .find_subcommand("api")
        .expect("api subcommand should be registered");
    assert!(api.get_arguments().any(|arg| arg.get_id() == "bind"));
    assert!(
        api.get_arguments()
            .any(|arg| arg.get_id() == "allow_non_loopback")
    );
}
