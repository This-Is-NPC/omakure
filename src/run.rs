use crate::adapters::script_runner::MultiScriptRunner;
use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::history;
use crate::ports::ScriptRunOutput;
use crate::runtime::script_extensions;
use crate::use_cases::ScriptService;
use crate::workspace::Workspace;
use std::error::Error;
use std::path::{Path, PathBuf};

pub struct RunOptions {
    pub script: String,
    pub args: Vec<String>,
    pub scripts_dir: PathBuf,
}

pub fn print_run_help() {
    println!(
        "Usage: omakure run <script> [--] [args...]\n\n\
Examples:\n\
  omakure run rg-list-all\n\
  omakure run tools/cleanup\n\
  omakure run scripts/cleanup.py -- --force\n\n\
Notes:\n\
  Script paths are relative to the workspace root.\n\
  Extensions supported: .bash, .sh, .ps1, .py\n\n\
Environment:\n\
  OMAKURE_SCRIPTS_DIR  Scripts directory override\n\
  OVERTURE_SCRIPTS_DIR  Legacy scripts directory override\n\
  CLOUD_MGMT_SCRIPTS_DIR  Legacy scripts directory override"
    );
}

pub fn wants_help(args: &[String]) -> bool {
    for arg in args {
        if arg == "--" {
            break;
        }
        if arg == "-h" || arg == "--help" {
            return true;
        }
    }
    false
}

pub fn parse_run_args(
    args: &[String],
    scripts_dir: PathBuf,
) -> Result<RunOptions, Box<dyn Error>> {
    if args.is_empty() {
        return Err("Missing script name. Use `omakure run <script>`.".into());
    }

    let script = args[0].clone();
    let remaining = &args[1..];
    let mut passthrough = Vec::new();
    let mut skip = false;

    for arg in remaining {
        if !skip && arg == "--" {
            skip = true;
            continue;
        }
        passthrough.push(arg.clone());
    }

    Ok(RunOptions {
        script,
        args: passthrough,
        scripts_dir,
    })
}

pub fn run_script(options: RunOptions) -> Result<(), Box<dyn Error>> {
    let workspace = Workspace::new(options.scripts_dir.clone());
    workspace.ensure_layout()?;

    let script_path = resolve_script_path(&options.script, workspace.root())?;

    let repo = Box::new(FsWorkspaceRepository::new(workspace.root().to_path_buf()));
    let runner = Box::new(MultiScriptRunner::new());
    let service = ScriptService::new(repo, runner);

    let run_result = service.run_script(&script_path, &options.args);
    match run_result {
        Ok(output) => {
            let success = output.success;
            let exit_code = output.exit_code.unwrap_or(1);
            print_output(&output);
            let entry = history::success_entry(&workspace, &script_path, &options.args, output);
            let _ = history::record_entry(&workspace, &entry);
            if !success {
                std::process::exit(exit_code);
            }
        }
        Err(err) => {
            eprintln!("{}", err);
            let entry =
                history::error_entry(&workspace, &script_path, &options.args, err.to_string());
            let _ = history::record_entry(&workspace, &entry);
            return Err(err);
        }
    }

    Ok(())
}

fn resolve_script_path(script: &str, scripts_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let has_separator = script.contains('/') || script.contains('\\');
    let path = PathBuf::from(script);

    if path.is_absolute() {
        return resolve_with_extensions(path);
    }

    if has_separator {
        return resolve_with_extensions(scripts_dir.join(path));
    }

    resolve_with_extensions(scripts_dir.join(script))
}

fn resolve_with_extensions(path: PathBuf) -> Result<PathBuf, Box<dyn Error>> {
    if path.exists() {
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("Script is not a file: {}", path.display()).into());
    }
    if path.extension().is_some() {
        return Err(format!("Script not found: {}", path.display()).into());
    }
    for ext in script_extensions() {
        let mut candidate = path.clone();
        candidate.set_extension(ext);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!("Script not found: {}", path.display()).into())
}

fn print_output(output: &ScriptRunOutput) {
    if !output.stdout.trim().is_empty() {
        print!("{}", output.stdout);
        if !output.stdout.ends_with('\n') {
            println!();
        }
    }
    if !output.stderr.trim().is_empty() {
        eprint!("{}", output.stderr);
        if !output.stderr.ends_with('\n') {
            eprintln!();
        }
    }
}
