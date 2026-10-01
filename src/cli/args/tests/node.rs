use super::*;

#[test]
fn test_parse_node_serve_flags() {
    let cli = parse(&[
        "node",
        "serve",
        "--bind",
        "127.0.0.1:8787",
        "--workers",
        "2",
        "--scheduler",
        "--readiness-requires-worker",
        "--readiness-requires-scheduler",
        "--allow-non-loopback-direct",
        "--worker-actor-filter",
        "agent",
        "--worker-script-filter",
        "tools/",
        "--secret-ref",
        "secret://env/*",
    ])
    .unwrap();
    match cli.command.unwrap() {
        Commands::Node(args) => match args.command {
            NodeCommand::Serve(args) => {
                assert_eq!(args.bind.unwrap().to_string(), "127.0.0.1:8787");
                assert_eq!(args.workers, Some(2));
                assert!(args.scheduler);
                assert!(!args.no_scheduler);
                assert!(args.readiness_requires_worker);
                assert!(args.readiness_requires_scheduler);
                assert!(args.allow_non_loopback_direct);
                assert_eq!(args.worker_actor_filter.as_deref(), Some("agent"));
                assert_eq!(args.worker_script_filter.as_deref(), Some("tools/"));
                assert_eq!(args.secret_refs, vec!["secret://env/*".to_string()]);
            }
            _ => panic!("expected node serve"),
        },
        _ => panic!("expected Node"),
    }
}

#[test]
fn node_serve_direct_non_loopback_flag_is_separate_from_http_flag() {
    let cli = parse(&["node", "serve", "--allow-non-loopback-direct"]).unwrap();
    match cli.command.unwrap() {
        Commands::Node(args) => match args.command {
            NodeCommand::Serve(args) => {
                assert!(args.allow_non_loopback_direct);
                assert!(!args.allow_non_loopback);
            }
            _ => panic!("expected node serve"),
        },
        _ => panic!("expected Node"),
    }
}

#[test]
fn test_node_reset_requires_explicit_flag_in_surface() {
    let cli = parse(&["node", "reset", "--confirmed"]).unwrap();
    match cli.command.unwrap() {
        Commands::Node(args) => match args.command {
            NodeCommand::Reset(args) => assert!(args.confirmed),
            _ => panic!("expected node reset"),
        },
        _ => panic!("expected Node"),
    }
}

#[test]
fn test_node_serve_defaults_are_safe() {
    let cli = parse(&["node", "serve"]).unwrap();
    match cli.command.unwrap() {
        Commands::Node(args) => match args.command {
            NodeCommand::Serve(args) => {
                assert!(args.bind.is_none());
                assert_eq!(args.workers, None);
                assert!(!args.scheduler);
                assert!(!args.no_scheduler);
                assert!(!args.readiness_requires_worker);
                assert!(!args.readiness_requires_scheduler);
                assert!(args.secret_refs.is_empty());
            }
            _ => panic!("expected node serve"),
        },
        _ => panic!("expected Node"),
    }
}

#[test]
fn test_node_serve_help_surface_exists() {
    let command = Cli::command();
    let node = command
        .find_subcommand("node")
        .expect("node subcommand should be registered");
    let serve = node
        .find_subcommand("serve")
        .expect("node serve should be registered");
    assert!(serve.get_arguments().any(|arg| arg.get_id() == "workers"));
    assert!(
        serve
            .get_arguments()
            .any(|arg| arg.get_id() == "readiness_requires_worker")
    );
}

#[test]
fn test_parse_node_serve_policy_flag() {
    let cli = parse(&["node", "serve", "--policy", "/tmp/p.toml"]).unwrap();
    match cli.command.unwrap() {
        Commands::Node(args) => match args.command {
            NodeCommand::Serve(args) => assert_eq!(
                args.policy.as_deref(),
                Some(std::path::Path::new("/tmp/p.toml"))
            ),
            _ => panic!("expected node serve"),
        },
        _ => panic!("expected Node"),
    }
}

#[test]
fn test_parse_node_cue_cue_id() {
    let without = parse(&[
        "node",
        "cue",
        "--endpoint",
        "127.0.0.1:7879",
        "--peer-node-id",
        "omk1_peer",
        "--script",
        "deploy.sh",
        "--reason",
        "roll out",
    ])
    .unwrap();
    match without.command.unwrap() {
        Commands::Node(args) => match args.command {
            NodeCommand::Cue(args) => assert_eq!(args.cue_id, None),
            _ => panic!("expected node cue"),
        },
        _ => panic!("expected Node"),
    }

    let with_id = parse(&[
        "node",
        "cue",
        "--endpoint",
        "127.0.0.1:7879",
        "--peer-node-id",
        "omk1_peer",
        "--script",
        "deploy.sh",
        "--reason",
        "roll out",
        "--cue-id",
        "0123456789abcdef0123456789abcdef",
    ])
    .unwrap();
    match with_id.command.unwrap() {
        Commands::Node(args) => match args.command {
            NodeCommand::Cue(args) => assert_eq!(
                args.cue_id.as_deref(),
                Some("0123456789abcdef0123456789abcdef")
            ),
            _ => panic!("expected node cue"),
        },
        _ => panic!("expected Node"),
    }
}
