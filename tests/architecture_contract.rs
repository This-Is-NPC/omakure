use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{
    ExprCall, ExprMethodCall, ExprPath, ImplItemFn, ItemFn, ItemUse, Lit, Path as SynPath, UseTree,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Rule {
    #[default]
    Http,
    HttpNodeDelivery,
    Catalog,
    RunStoreBoundary,
    Domain,
    RunsSql,
    Executor,
    GitProcess,
    HealthLifecycle,
    HealthProcess,
    BatteryFilesystem,
    OperationInput,
    SearchStorage,
}

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    rule: &'static str,
    symbol: String,
}

impl Finding {
    fn diagnostic(&self, path: &str) -> String {
        format!("{}: {}:{}", self.rule, path, self.symbol)
    }
}
#[derive(Default)]
struct ContractVisitor {
    rule: Rule,
    symbol: String,
    findings: Vec<Finding>,
    executor_calls: Vec<String>,
    executor_aliases: HashMap<String, String>,
    executor_modules: HashSet<String>,
}

impl ContractVisitor {
    fn record(&mut self, rule: &'static str, _detail: &'static str) {
        self.findings.push(Finding {
            rule,
            symbol: self.symbol.clone(),
        });
    }

    fn path_segments(path: &SynPath) -> Vec<String> {
        path.segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect()
    }

    fn starts_with(path: &[String], prefix: &[&str]) -> bool {
        path.len() >= prefix.len()
            && path
                .iter()
                .zip(prefix)
                .all(|(segment, expected)| segment == expected)
    }

    fn check_import(&mut self, path: &[String]) {
        if self.rule == Rule::Http
            && Self::starts_with(path, &["crate", "cli"])
            && !Self::starts_with(path, &["crate", "cli", "args"])
            && !Self::starts_with(path, &["crate", "cli", "json"])
        {
            self.record(
                "ARCH-HTTP-CLI",
                "HTTP must call operations, not CLI adapters",
            );
        }
        if self.rule == Rule::Catalog && Self::starts_with(path, &["crate", "cli"]) {
            self.record("ARCH-CATALOG-CLI", "catalogs must use shared inventories");
        }
        if self.rule == Rule::SearchStorage && Self::starts_with(path, &["crate", "operations"]) {
            self.record(
                "ARCH-SEARCH-OPERATIONS",
                "search storage must not depend on operations",
            );
        }
        if self.rule == Rule::OperationInput
            && (Self::starts_with(path, &["crate", "cli"])
                || Self::starts_with(path, &["crate", "node"])
                || Self::starts_with(path, &["crate", "node_identity"])
                || matches!(path.first().map(String::as_str), Some("axum" | "clap")))
        {
            self.record(
                "ARCH-OPERATION-INPUT",
                "core and Health operations must receive protocol-neutral inputs",
            );
        }
        if self.rule == Rule::HealthLifecycle
            && Self::starts_with(path, &["crate", "node_registry"])
        {
            self.record(
                "ARCH-HEALTH-LIFECYCLE-REGISTRY",
                "lifecycle projection must consume domain transition values",
            );
        }
        if self.rule == Rule::HealthProcess && Self::starts_with(path, &["std", "process"]) {
            self.record(
                "ARCH-HEALTH-PROCESS",
                "runtime process execution belongs in adapters/system_checks",
            );
        }
        if self.rule == Rule::Http && path.first().map(String::as_str) == Some("rusqlite") {
            self.record("ARCH-HTTP-SQLITE", "HTTP must not access SQLite directly");
        }
        if self.rule == Rule::HttpNodeDelivery
            && [
                ["crate", "remote_cue"],
                ["crate", "baseline_push"],
                ["crate", "direct_service"],
            ]
            .into_iter()
            .any(|prefix| Self::starts_with(path, &prefix))
        {
            self.record(
                "ARCH-HTTP-NODE-DELIVERY",
                "node delivery handlers must call operations",
            );
        }
        if self.rule == Rule::RunStoreBoundary
            && (path.first().map(String::as_str) == Some("rusqlite")
                || Self::starts_with(path, &["runs", "open"])
                || Self::starts_with(path, &["crate", "runs", "open"]))
        {
            self.record(
                "ARCH-RUN-STORE-BOUNDARY",
                "run operations must use the opaque runs store",
            );
        }
        if self.rule == Rule::Domain
            && (Self::starts_with(path, &["std", "fs"])
                || Self::starts_with(path, &["std", "process"])
                || path.first().map(String::as_str) == Some("tokio")
                || Self::starts_with(path, &["crate", "adapters"])
                || Self::starts_with(path, &["crate", "run_executor"])
                || Self::starts_with(path, &["crate", "runtime"]))
        {
            self.record(
                "ARCH-DOMAIN-IO",
                "domain must not depend on I/O, runtimes, or adapters",
            );
        }
    }
}

impl<'ast> Visit<'ast> for ContractVisitor {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let previous = std::mem::replace(&mut self.symbol, node.sig.ident.to_string());
        visit::visit_item_fn(self, node);
        self.symbol = previous;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        let previous = std::mem::replace(&mut self.symbol, node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.symbol = previous;
    }

    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        for (path, alias) in use_tree_imports(&node.tree) {
            self.check_import(&path);
            if self.rule == Rule::Executor
                && ContractVisitor::starts_with(&path, &["crate", "run_executor"])
            {
                if path.len() == 2 {
                    if let Some(alias) = alias {
                        self.executor_modules.insert(alias);
                    }
                } else if let Some(alias) = alias {
                    self.executor_aliases
                        .insert(alias, path.last().cloned().unwrap_or_default());
                }
            }
        }
        visit::visit_item_use(self, node);
    }

    fn visit_path(&mut self, path: &'ast SynPath) {
        let segments = Self::path_segments(path);
        self.check_import(&segments);
        visit::visit_path(self, path);
    }

    fn visit_lit(&mut self, literal: &'ast Lit) {
        if self.rule == Rule::RunsSql
            && let Lit::Str(value) = literal
            && contains_run_table_sql(&value.value())
        {
            self.record("ARCH-RUNS-SQL", "run-table SQL belongs only to runs/");
        }
        visit::visit_lit(self, literal);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if self.rule == Rule::RunsSql {
            // Macro token streams split SQL literals across arguments. Joining
            // their textual fragments catches concat!/format! forms while
            // remaining conservative for non-SQL macros.
            let fragments = node.tokens.to_string().replace('"', " ");
            if contains_run_table_sql(&fragments) {
                self.record("ARCH-RUNS-SQL", "run-table SQL belongs only to runs/");
            }
        }
        visit::visit_macro(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if self.rule == Rule::HealthProcess
            && let syn::Expr::Path(ExprPath { path, .. }) = node.func.as_ref()
            && Self::path_segments(path).ends_with(&["Command".into(), "new".into()])
        {
            self.record(
                "ARCH-HEALTH-PROCESS",
                "runtime process execution belongs in adapters/system_checks",
            );
        }
        if self.rule == Rule::BatteryFilesystem
            && let syn::Expr::Path(ExprPath { path, .. }) = node.func.as_ref()
        {
            let segments = Self::path_segments(path);
            if segments.first().map(String::as_str) == Some("libc")
                && matches!(
                    segments.last().map(String::as_str),
                    Some("open" | "openat" | "renameat" | "linkat" | "unlinkat")
                )
            {
                self.record(
                    "ARCH-BATTERY-FS",
                    "raw filesystem calls belong in adapters/fs",
                );
            }
        }
        if self.rule == Rule::GitProcess
            && let syn::Expr::Path(ExprPath { path, .. }) = node.func.as_ref()
            && Self::path_segments(path).ends_with(&["Command".into(), "new".into()])
        {
            self.record(
                "ARCH-GIT-PROCESS",
                "Git subprocesses belong in adapters/git",
            );
        }
        if self.rule == Rule::Executor
            && let syn::Expr::Path(ExprPath { path, .. }) = node.func.as_ref()
        {
            let Some(name) = path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            else {
                visit::visit_expr_call(self, node);
                return;
            };
            let direct_executor = path
                .segments
                .iter()
                .any(|segment| segment.ident == "run_executor");
            let module_alias = path
                .segments
                .first()
                .map(|segment| self.executor_modules.contains(&segment.ident.to_string()))
                .unwrap_or(false);
            let imported_target = self.executor_aliases.get(&name).cloned();
            let from_executor = direct_executor || module_alias || imported_target.is_some();
            if from_executor {
                let target = imported_target.unwrap_or_else(|| name.clone());
                self.executor_calls.push(target.clone());
                if !matches!(
                    target.as_str(),
                    "execute_with_heartbeat" | "execute_with_heartbeat_guarded"
                ) {
                    self.record(
                        "ARCH-EXECUTOR-CONVERGENCE",
                        "direct and queued runs must use the heartbeat executor",
                    );
                }
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
        if self.rule == Rule::BatteryFilesystem {
            self.record(
                "ARCH-BATTERY-FS",
                "unsafe filesystem calls belong in adapters/fs",
            );
        }
        visit::visit_expr_unsafe(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if self.rule == Rule::GitProcess
            && matches!(
                node.method.to_string().as_str(),
                "spawn" | "output" | "status"
            )
        {
            self.record(
                "ARCH-GIT-PROCESS",
                "Git subprocesses belong in adapters/git",
            );
        }
        visit::visit_expr_method_call(self, node);
    }
}

/// Flatten `use` trees into source paths and their local names. `UseTree`
/// identifiers are not visited as `syn::Path`, so boundary rules must inspect
/// these paths explicitly.
fn use_tree_imports(tree: &UseTree) -> Vec<(Vec<String>, Option<String>)> {
    fn visit(
        tree: &UseTree,
        prefix: &mut Vec<String>,
        imports: &mut Vec<(Vec<String>, Option<String>)>,
    ) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                visit(&path.tree, prefix, imports);
                prefix.pop();
            }
            UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                imports.push((prefix.clone(), Some(name.ident.to_string())));
                prefix.pop();
            }
            UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                imports.push((prefix.clone(), Some(rename.rename.to_string())));
                prefix.pop();
            }
            UseTree::Glob(_) => imports.push((prefix.clone(), None)),
            UseTree::Group(group) => {
                for item in &group.items {
                    visit(item, prefix, imports);
                }
            }
        }
    }

    let mut imports = Vec::new();
    visit(tree, &mut Vec::new(), &mut imports);
    imports
}
fn parse_contract(rule: Rule, path: &str, source: &str) -> ContractVisitor {
    let syntax =
        syn::parse_file(source).unwrap_or_else(|error| panic!("{path} must parse: {error}"));
    let mut visitor = ContractVisitor {
        rule,
        symbol: "<module>".into(),
        ..Default::default()
    };
    visitor.visit_file(&syntax);
    visitor
}

fn contains_run_table_sql(sql: &str) -> bool {
    for statement in sql.to_ascii_lowercase().split(';') {
        let statement = statement
            .lines()
            .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n");
        let tokens: Vec<_> = statement
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .filter(|token| !token.is_empty())
            .collect();
        let Some(first) = tokens.first().copied() else {
            continue;
        };
        if !matches!(
            first,
            "select"
                | "insert"
                | "update"
                | "delete"
                | "create"
                | "drop"
                | "alter"
                | "replace"
                | "with"
        ) {
            continue;
        }
        if tokens.windows(2).any(|window| {
            matches!(window[0], "from" | "into" | "update" | "join" | "table")
                && window[1] == "runs"
        }) {
            return true;
        }
        if first == "create"
            && tokens
                .iter()
                .position(|token| *token == "table")
                .is_some_and(|index| tokens.iter().skip(index + 1).any(|token| *token == "runs"))
        {
            return true;
        }
    }
    false
}

fn fixture(name: &str) -> (&'static str, &'static str) {
    match name {
        "positive_http" => (
            "fixture:positive_http.rs",
            include_str!("fixtures/architecture/positive_http.rs"),
        ),
        "negative_http" => (
            "fixture:negative_http.rs",
            include_str!("fixtures/architecture/negative_http.rs"),
        ),
        "positive_domain" => (
            "fixture:positive_domain.rs",
            include_str!("fixtures/architecture/positive_domain.rs"),
        ),
        "negative_domain" => (
            "fixture:negative_domain.rs",
            include_str!("fixtures/architecture/negative_domain.rs"),
        ),
        "positive_runs_sql" => (
            "fixture:positive_runs_sql.rs",
            include_str!("fixtures/architecture/positive_runs_sql.rs"),
        ),
        "negative_runs_sql" => (
            "fixture:negative_runs_sql.rs",
            include_str!("fixtures/architecture/negative_runs_sql.rs"),
        ),
        "positive_executor" => (
            "fixture:positive_executor.rs",
            include_str!("fixtures/architecture/positive_executor.rs"),
        ),
        "negative_executor" => (
            "fixture:negative_executor.rs",
            include_str!("fixtures/architecture/negative_executor.rs"),
        ),
        _ => panic!("unknown architecture fixture {name}"),
    }
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::metadata(&path).expect("architecture source metadata");
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).expect("architecture source directory") {
                pending.push(entry.expect("architecture source entry").path());
            }
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn seeded_architecture_fixtures_have_stable_positive_and_negative_results() {
    for (name, rule) in [
        ("positive_http", Rule::Http),
        ("positive_domain", Rule::Domain),
        ("positive_runs_sql", Rule::RunsSql),
        ("positive_executor", Rule::Executor),
    ] {
        let (_, source) = fixture(name);
        assert!(
            parse_contract(rule, name, source).findings.is_empty(),
            "{name} must be allowed"
        );
    }

    let cases = [
        ("negative_http", Rule::Http, "ARCH-HTTP-CLI", "handler"),
        ("negative_http", Rule::Http, "ARCH-HTTP-SQLITE", "handler"),
        (
            "negative_domain",
            Rule::Domain,
            "ARCH-DOMAIN-IO",
            "load_schema",
        ),
        (
            "negative_runs_sql",
            Rule::RunsSql,
            "ARCH-RUNS-SQL",
            "inspect",
        ),
        (
            "negative_executor",
            Rule::Executor,
            "ARCH-EXECUTOR-CONVERGENCE",
            "run_direct",
        ),
    ];
    for (name, rule, expected_rule, symbol) in cases {
        let (path, source) = fixture(name);
        let findings = parse_contract(rule, name, source).findings;
        let diagnostics: Vec<_> = findings
            .iter()
            .map(|finding| finding.diagnostic(path))
            .collect();
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic == &format!("{expected_rule}: {path}:{symbol}")),
            "{name} must report stable {expected_rule} diagnostic; got {diagnostics:?}"
        );
    }
}

#[test]
fn production_architecture_boundaries_are_clean() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("src");

    let lifecycle = fs::read_to_string(src.join("health_plane/lifecycle.rs")).unwrap();
    let lifecycle_contract = parse_contract(
        Rule::HealthLifecycle,
        "src/health_plane/lifecycle.rs",
        &lifecycle,
    );
    assert!(
        lifecycle_contract.findings.is_empty(),
        "Health lifecycle projection depends on registry: {:?}",
        lifecycle_contract.findings
    );

    let battery_git = fs::read_to_string(src.join("operations/battery/git.rs")).unwrap();
    let git_process = parse_contract(
        Rule::GitProcess,
        "src/operations/battery/git.rs",
        &battery_git,
    );
    assert!(
        git_process.findings.is_empty(),
        "Git process calls must stay in adapters/git: {:?}",
        git_process.findings
    );

    for path in source_files(&src.join("cli/api")) {
        let display = path.strip_prefix(root).unwrap().display().to_string();
        let source = fs::read_to_string(&path).expect("read HTTP adapter");
        let contract = parse_contract(Rule::Http, &display, &source);
        assert!(
            contract
                .findings
                .iter()
                .all(|finding| { !matches!(finding.rule, "ARCH-HTTP-CLI" | "ARCH-HTTP-SQLITE") }),
            "HTTP boundary violations in {display}: {:?}",
            contract.findings
        );
    }

    let node_handler_path = src.join("cli/api/node.rs");
    let node_handler = fs::read_to_string(&node_handler_path).expect("read node HTTP handler");
    let delivery = parse_contract(Rule::HttpNodeDelivery, "src/cli/api/node.rs", &node_handler);
    assert!(
        delivery.findings.is_empty(),
        "node delivery HTTP boundary violations: {:?}",
        delivery.findings
    );
    for operation in [
        "cue_ops::prepare_service_dispatch",
        "cue_ops::dispatch_prepared_service",
        "baseline_ops::prepare_service_push",
        "baseline_ops::push_prepared_service",
        "baseline_ops::rollback_local_baseline",
    ] {
        assert!(
            node_handler.contains(operation),
            "node HTTP handler must call {operation}"
        );
    }

    for path in source_files(&src.join("domain")) {
        let display = path.strip_prefix(root).unwrap().display().to_string();
        let source = fs::read_to_string(&path).expect("read domain source");
        let contract = parse_contract(Rule::Domain, &display, &source);
        assert!(
            contract
                .findings
                .iter()
                .all(|finding| finding.rule != "ARCH-DOMAIN-IO"),
            "domain boundary violations in {display}: {:?}",
            contract.findings
        );
    }

    for path in source_files(&src) {
        if path.starts_with(src.join("runs")) {
            continue;
        }
        let display = path.strip_prefix(root).unwrap().display().to_string();
        let source = fs::read_to_string(&path).expect("read source");
        let contract = parse_contract(Rule::RunsSql, &display, &source);
        assert!(
            contract
                .findings
                .iter()
                .all(|finding| finding.rule != "ARCH-RUNS-SQL"),
            "run-table SQL outside runs/ in {display}: {:?}",
            contract.findings
        );
    }

    for relative in [
        "operations/core/run_queries.rs",
        "operations/core/enqueue.rs",
        "operations/health/facts.rs",
        "operations/node/trust.rs",
        "operations/worker.rs",
        "cli/serve/scheduler.rs",
        "cli/run/mod.rs",
        "cli/trace.rs",
    ] {
        let path = src.join(relative);
        let source = fs::read_to_string(&path).expect("read run operation source");
        let contract = parse_contract(Rule::RunStoreBoundary, relative, &source);
        assert!(
            contract.findings.is_empty(),
            "run operations must use the opaque runs store in {relative}: {:?}",
            contract.findings
        );
        assert!(
            source.contains("RunStore::open("),
            "{relative} must open RunStore"
        );
    }

    let direct = parse_contract(
        Rule::Executor,
        "src/cli/run/mod.rs",
        &fs::read_to_string(src.join("cli/run/mod.rs")).unwrap(),
    );
    assert!(
        direct
            .findings
            .iter()
            .all(|finding| finding.rule != "ARCH-EXECUTOR-CONVERGENCE")
    );
    assert!(
        direct
            .executor_calls
            .iter()
            .any(|call| call == "execute_with_heartbeat")
    );

    let worker = parse_contract(
        Rule::Executor,
        "src/operations/worker.rs",
        &fs::read_to_string(src.join("operations/worker.rs")).unwrap(),
    );
    assert!(
        worker
            .findings
            .iter()
            .all(|finding| finding.rule != "ARCH-EXECUTOR-CONVERGENCE")
    );
    assert!(
        worker
            .executor_calls
            .iter()
            .any(|call| call == "execute_with_heartbeat_guarded")
    );

    let node_service = fs::read_to_string(src.join("cli/node_service.rs")).unwrap();
    let storage = parse_contract(
        Rule::RunStoreBoundary,
        "src/cli/node_service.rs",
        &node_service,
    );
    assert!(storage.findings.is_empty(), "{:?}", storage.findings);
    assert!(node_service.contains("operations::worker::recover_abandoned_remote_runs("));

    let queue_adapter = fs::read_to_string(src.join("cli/queue/mod.rs")).unwrap();
    assert!(queue_adapter.contains("run_standalone_workers("));
    assert!(!queue_adapter.contains("install_signal_handlers("));
    assert!(!queue_adapter.contains("thread::spawn("));

    let scheduler = parse_contract(
        Rule::Executor,
        "src/cli/serve/scheduler.rs",
        &fs::read_to_string(src.join("cli/serve/scheduler.rs")).unwrap(),
    );
    assert!(
        scheduler
            .findings
            .iter()
            .all(|finding| finding.rule != "ARCH-EXECUTOR-CONVERGENCE")
    );
    assert!(
        scheduler.executor_calls.is_empty(),
        "scheduler must enqueue only"
    );
}

#[test]
fn node_delivery_contract_rejects_direct_protocol_calls() {
    for module in ["remote_cue", "baseline_push", "direct_service"] {
        let source = format!("fn handler() {{ let _ = crate::{module}::read_policy; }}");
        let contract = parse_contract(Rule::HttpNodeDelivery, "fixture:node.rs", &source);
        assert!(
            contract
                .findings
                .iter()
                .any(|finding| finding.rule == "ARCH-HTTP-NODE-DELIVERY"),
            "direct {module} call was not rejected"
        );
    }
}

#[test]
fn shared_catalogs_do_not_depend_on_cli_adapters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for module in ["inventory", "cli_http_parity", "operation_catalog"] {
        for path in source_files(&root.join("src").join(module)) {
            let display = path.strip_prefix(root).unwrap().display().to_string();
            let source = fs::read_to_string(&path).expect("read catalog source");
            let contract = parse_contract(Rule::Catalog, &display, &source);
            assert!(
                contract.findings.is_empty(),
                "catalog boundary violations in {display}: {:?}",
                contract.findings
            );
        }
    }
    let contract = parse_contract(
        Rule::Catalog,
        "fixture:catalog.rs",
        "use crate::cli::args::Cli;",
    );
    assert_eq!(contract.findings.len(), 1);
    assert_eq!(contract.findings[0].rule, "ARCH-CATALOG-CLI");
}

#[test]
fn search_storage_does_not_depend_on_operations() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = root.join("src/search_index.rs");
    let source = fs::read_to_string(&path).expect("read search storage source");
    let contract = parse_contract(Rule::SearchStorage, "src/search_index.rs", &source);
    assert!(
        contract.findings.is_empty(),
        "search storage boundary violations: {:?}",
        contract.findings
    );

    let contract = parse_contract(
        Rule::SearchStorage,
        "fixture:search_index.rs",
        "use crate::operations::path::logical_relative_path;",
    );
    assert_eq!(contract.findings.len(), 1);
    assert_eq!(contract.findings[0].rule, "ARCH-SEARCH-OPERATIONS");
}

#[test]
fn core_and_health_operations_receive_protocol_neutral_inputs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for module in ["operations/core", "operations/health"] {
        for path in source_files(&root.join("src").join(module)) {
            let display = path.strip_prefix(root).unwrap().display().to_string();
            let source = fs::read_to_string(&path).expect("read operation source");
            let contract = parse_contract(Rule::OperationInput, &display, &source);
            assert!(
                contract.findings.is_empty(),
                "operation input boundary violations in {display}: {:?}",
                contract.findings
            );
        }
    }
    for import in [
        "use crate::cli::args::Cli;",
        "use crate::node::NodeContext;",
        "use crate::node_identity::NodeIdentity;",
        "use axum::extract::Query;",
    ] {
        let contract = parse_contract(Rule::OperationInput, "fixture:input.rs", import);
        assert_eq!(contract.findings.len(), 1, "{import}");
    }
}

#[test]
fn run_operations_reject_direct_storage_handles() {
    for source in [
        "fn require_run(conn: &rusqlite::Connection) {}",
        "fn open_run_store(workspace: &Workspace) { runs::open(workspace); }",
    ] {
        let contract = parse_contract(Rule::RunStoreBoundary, "fixture:run_operation.rs", source);
        assert_eq!(contract.findings.len(), 1);
        assert_eq!(contract.findings[0].rule, "ARCH-RUN-STORE-BOUNDARY");
    }
}

#[test]
fn battery_git_operations_reject_process_execution() {
    let contract = parse_contract(
        Rule::GitProcess,
        "fixture:battery_git.rs",
        "fn run() { let _ = std::process::Command::new(\"git\").output(); }",
    );
    assert!(
        contract
            .findings
            .iter()
            .any(|finding| finding.rule == "ARCH-GIT-PROCESS")
    );
}

#[test]
fn health_facts_process_execution_stays_in_adapter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = "src/operations/health/facts.rs";
    let source = fs::read_to_string(root.join(path)).expect("read Health facts source");
    let contract = parse_contract(Rule::HealthProcess, path, &source);
    assert!(
        contract.findings.is_empty(),
        "Health facts must not launch processes: {:?}",
        contract.findings
    );
    for source in [
        "use std::process::Command;",
        "fn probe() { Command::new(\"bash\"); }",
    ] {
        let contract = parse_contract(Rule::HealthProcess, "fixture:health.rs", source);
        assert_eq!(contract.findings.len(), 1);
        assert_eq!(contract.findings[0].rule, "ARCH-HEALTH-PROCESS");
    }
}

#[test]
fn battery_filesystem_syscalls_stay_in_adapters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in source_files(&root.join("src/operations/battery")) {
        let display = path.strip_prefix(root).unwrap().display().to_string();
        let source = fs::read_to_string(&path).expect("read Battery operation source");
        let contract = parse_contract(Rule::BatteryFilesystem, &display, &source);
        assert!(
            contract.findings.is_empty(),
            "Battery filesystem boundary violations in {display}: {:?}",
            contract.findings
        );
    }
    for source in [
        "fn install() { unsafe { libc::open(std::ptr::null(), 0); } }",
        "fn install() { libc::renameat(0, std::ptr::null(), 0, std::ptr::null()); }",
    ] {
        let contract = parse_contract(Rule::BatteryFilesystem, "fixture:battery.rs", source);
        assert!(
            contract
                .findings
                .iter()
                .any(|finding| finding.rule == "ARCH-BATTERY-FS")
        );
    }
}

#[test]
fn internal_security_modules_stay_crate_scoped() {
    let source = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("read crate surface");
    let syntax = syn::parse_file(&source).expect("parse crate surface");
    let mut expected: HashSet<&str> = [
        "auth",
        "baseline_publisher",
        "enrollment_authority",
        "redaction",
        "secrets",
    ]
    .into_iter()
    .collect();
    for item in syntax.items {
        let syn::Item::Mod(module) = item else {
            continue;
        };
        let name = module.ident.to_string();
        if expected.remove(name.as_str()) {
            assert!(
                matches!(module.vis, syn::Visibility::Restricted(ref visibility) if visibility.path.is_ident("crate")),
                "{name} must remain crate scoped"
            );
        }
    }
    assert!(
        expected.is_empty(),
        "internal modules missing: {expected:?}"
    );
}

#[test]
fn lifecycle_projection_rejects_registry_types() {
    let contract = parse_contract(
        Rule::HealthLifecycle,
        "fixture:lifecycle.rs",
        "use crate::node_registry::AuditEvent;",
    );
    assert_eq!(contract.findings.len(), 1);
    assert_eq!(contract.findings[0].rule, "ARCH-HEALTH-LIFECYCLE-REGISTRY");
}
