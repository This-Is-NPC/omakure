use crate::adapters::workspace_repository::FsWorkspaceRepository;
use crate::ports::ScriptRepository;
use crate::workspace::Workspace;
use std::path::Path;

/// Verify that every required field on the script's schema has its
/// `--<field>` (or `Arg` override) among `args`. Returns
/// `Err((field_name, message))` for the first missing one. A script without a
/// readable schema passes: it may validate its own input.
pub(crate) fn check_required_fields(
    workspace: &Workspace,
    script: &Path,
    args: &[String],
) -> Result<(), (String, String)> {
    let repo = FsWorkspaceRepository::new(workspace.root().to_path_buf());
    let Ok(schema) = repo.read_schema(script) else {
        return Ok(());
    };
    for field in &schema.fields {
        if !field.required.unwrap_or(false) {
            continue;
        }
        let arg_flag = field
            .arg
            .clone()
            .unwrap_or_else(|| format!("--{}", field.name));
        if !args_contain_flag(args, &arg_flag) {
            return Err((
                field.name.clone(),
                format!("expected `{}` on the command line", arg_flag),
            ));
        }
    }
    Ok(())
}

pub(crate) fn args_contain_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{}=", flag)))
}
