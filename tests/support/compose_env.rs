use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ComposeEnv {
    target_tokens: PathBuf,
    target_client: PathBuf,
    candidate_tokens: PathBuf,
    candidate_client: PathBuf,
}

impl ComposeEnv {
    pub fn new(
        target_tokens: PathBuf,
        target_client: PathBuf,
        candidate_tokens: PathBuf,
        candidate_client: PathBuf,
    ) -> Self {
        Self {
            target_tokens,
            target_client,
            candidate_tokens,
            candidate_client,
        }
    }

    pub fn client_file(&self, url: &str) -> &Path {
        if url.contains(":17879/") {
            &self.candidate_client
        } else {
            &self.target_client
        }
    }

    pub fn apply(&self, command: &mut Command) {
        command
            .env("OMAKURE_ENROLLMENT_TARGET_TOKENS_FILE", &self.target_tokens)
            .env("OMAKURE_ENROLLMENT_TARGET_CLIENT_FILE", &self.target_client)
            .env(
                "OMAKURE_ENROLLMENT_CANDIDATE_TOKENS_FILE",
                &self.candidate_tokens,
            )
            .env(
                "OMAKURE_ENROLLMENT_CANDIDATE_CLIENT_FILE",
                &self.candidate_client,
            );
    }
}

#[cfg(test)]
mod tests {
    use super::ComposeEnv;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::process::Command;

    #[test]
    fn compose_paths_are_confined_to_the_child_command() {
        let env = ComposeEnv::new(
            PathBuf::from("/test/target.tokens"),
            PathBuf::from("/test/target.client"),
            PathBuf::from("/test/candidate.tokens"),
            PathBuf::from("/test/candidate.client"),
        );
        let keys = [
            "OMAKURE_ENROLLMENT_TARGET_TOKENS_FILE",
            "OMAKURE_ENROLLMENT_TARGET_CLIENT_FILE",
            "OMAKURE_ENROLLMENT_CANDIDATE_TOKENS_FILE",
            "OMAKURE_ENROLLMENT_CANDIDATE_CLIENT_FILE",
        ];
        let inherited: Vec<_> = keys.iter().map(std::env::var_os).collect();
        let mut command = Command::new("docker");
        env.apply(&mut command);
        let injected: BTreeMap<_, _> = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(OsString::from)))
            .collect();
        assert_eq!(
            injected,
            keys.into_iter()
                .zip([
                    "/test/target.tokens",
                    "/test/target.client",
                    "/test/candidate.tokens",
                    "/test/candidate.client",
                ])
                .map(|(key, value)| (OsString::from(key), Some(OsString::from(value))))
                .collect::<BTreeMap<_, _>>()
        );
        assert_eq!(
            keys.iter().map(std::env::var_os).collect::<Vec<_>>(),
            inherited
        );
        assert_eq!(
            env.client_file("http://127.0.0.1:17879/v1/node/status"),
            PathBuf::from("/test/candidate.client")
        );
        assert_eq!(
            env.client_file("http://127.0.0.1:17878/v1/node/status"),
            PathBuf::from("/test/target.client")
        );
    }
}
