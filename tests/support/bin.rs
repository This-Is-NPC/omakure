use std::path::PathBuf;

pub fn omakure_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_omakure"))
}
