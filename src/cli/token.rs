//! `omakure token generate` — create Argon2id hashed bearer tokens.

use crate::auth;
use crate::cli::args::{TokenArgs, TokenCommand, TokenGenerateArgs};
use crate::cli::json;
use serde::Serialize;
use std::error::Error;

#[derive(Serialize)]
struct GenerateOutput {
    id: String,
    token: String,
    hash: String,
    scopes: Vec<String>,
    tokens_file_entry: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    appended_to: Option<String>,
}

pub fn run(args: TokenArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    match args.command {
        TokenCommand::Generate(generate) => run_generate(generate, json_output),
    }
}

fn run_generate(args: TokenGenerateArgs, json_output: bool) -> Result<(), Box<dyn Error>> {
    if args.append.is_some() && !args.confirmed {
        let msg = "--append requires --confirmed";
        if json_output {
            json::print_err(json::codes::INVALID_ARGUMENT, msg);
        }
        return Err(msg.into());
    }

    let generated = auth::generate_token(&args.id, &args.scopes)?;
    let mut appended_to: Option<String> = None;
    if let Some(path) = args.append.as_deref() {
        auth::append_token_entry(path, &generated.id, &generated.tokens_file_entry)?;
        appended_to = Some(path.display().to_string());
    }

    let output = GenerateOutput {
        id: generated.id,
        token: generated.token,
        hash: generated.hash,
        scopes: generated.scopes,
        tokens_file_entry: generated.tokens_file_entry,
        appended_to,
    };

    if json_output {
        json::print_ok(output);
    } else {
        println!("id: {}", output.id);
        println!("token: {}", output.token);
        println!("hash: {}", output.hash);
        println!("scopes: {}", output.scopes.join(", "));
        if let Some(path) = &output.appended_to {
            println!("appended_to: {path}");
        }
        println!();
        print!("{}", output.tokens_file_entry);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::TokenGenerateArgs;
    use tempfile::TempDir;

    #[test]
    fn generate_requires_confirmed_for_append() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        let err = run_generate(
            TokenGenerateArgs {
                id: "ci".into(),
                scopes: vec!["runs:read".into()],
                append: Some(path),
                confirmed: false,
            },
            true,
        )
        .unwrap_err();
        assert!(err.to_string().contains("--confirmed"));
    }

    #[test]
    fn generate_appends_with_confirmed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tokens.toml");
        run_generate(
            TokenGenerateArgs {
                id: "ci".into(),
                scopes: vec!["runs:read".into()],
                append: Some(path.clone()),
                confirmed: true,
            },
            true,
        )
        .unwrap();
        let tokens = auth::load_tokens_file(&path).unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].id, "ci");
    }
}
