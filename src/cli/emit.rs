//! Failure reporting shared by the CLI verbs.
//!
//! In `--json` mode a failure prints the error envelope on stdout and exits
//! with status 1 directly: returning `Err` would make `main` also print
//! `error: <msg>` on stderr, which agents reading `--json` must not see.

use crate::cli::json;
use crate::operations::OperationError;
use std::error::Error;

/// Report a failure under a stable [`json::codes`] code. Human mode returns
/// the message so `main` prints it and exits non-zero.
pub fn emit_error(
    json_output: bool,
    code: &str,
    message: impl Into<String>,
) -> Result<(), Box<dyn Error>> {
    let message = message.into();
    if json_output {
        json::print_err(code, message);
        std::process::exit(1);
    }
    Err(message.into())
}

/// Report an operation failure under the command-specific envelope code
/// chosen by `cli_code`.
pub fn emit_operation_error(
    json_output: bool,
    err: OperationError,
    cli_code: fn(&OperationError) -> &'static str,
) -> Result<(), Box<dyn Error>> {
    emit_error(json_output, cli_code(&err), err.message)
}

/// Report an operation failure under its own operation code. Human mode
/// returns the error itself, rendered as `<code>: <message>`.
pub fn emit_native_operation_error(
    json_output: bool,
    err: OperationError,
) -> Result<(), Box<dyn Error>> {
    if json_output {
        json::print_err(err.code.as_str(), err.message);
        std::process::exit(1);
    }
    Err(Box::new(err))
}

/// Report a failure and exit with status 1 in both modes; human mode prints
/// `error: <msg>` on stderr itself.
pub fn exit_with_error(json_output: bool, code: &str, message: impl Into<String>) -> ! {
    let message = message.into();
    if json_output {
        json::print_err(code, message);
    } else {
        eprintln!("error: {message}");
    }
    std::process::exit(1);
}
