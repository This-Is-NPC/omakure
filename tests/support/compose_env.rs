use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy)]
pub struct ComposePorts {
    pub target: u16,
    pub candidate: u16,
}

impl ComposePorts {
    pub fn from_environment() -> Self {
        fn port(name: &str, default: u16) -> u16 {
            match std::env::var(name) {
                Ok(value) => value
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port != 0)
                    .unwrap_or_else(|| panic!("{name} must be a nonzero TCP port")),
                Err(std::env::VarError::NotPresent) => default,
                Err(std::env::VarError::NotUnicode(_)) => {
                    panic!("{name} must be a nonzero TCP port")
                }
            }
        }

        let target = port("DIRECT_A_HTTP_PORT", 17878);
        let candidate = port("DIRECT_B_HTTP_PORT", 17879);
        assert_ne!(target, candidate, "Compose API ports must differ");
        Self { target, candidate }
    }
}

pub struct ComposeEnv {
    target_tokens: PathBuf,
    target_client: PathBuf,
    candidate_tokens: PathBuf,
    candidate_client: PathBuf,
    ports: ComposePorts,
}

impl ComposeEnv {
    pub fn new(
        target_tokens: PathBuf,
        target_client: PathBuf,
        candidate_tokens: PathBuf,
        candidate_client: PathBuf,
        ports: ComposePorts,
    ) -> Self {
        Self {
            target_tokens,
            target_client,
            candidate_tokens,
            candidate_client,
            ports,
        }
    }

    pub fn ports(&self) -> ComposePorts {
        self.ports
    }

    pub fn target_api(&self) -> String {
        format!("http://127.0.0.1:{}", self.ports.target)
    }

    pub fn candidate_api(&self) -> String {
        format!("http://127.0.0.1:{}", self.ports.candidate)
    }

    pub fn client_file(&self, url: &str) -> &Path {
        if url.starts_with(&format!("{}/", self.candidate_api())) {
            &self.candidate_client
        } else {
            assert!(
                url.starts_with(&format!("{}/", self.target_api())),
                "unexpected Compose API endpoint"
            );
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
            )
            .env("DIRECT_A_HTTP_PORT", self.ports.target.to_string())
            .env("DIRECT_B_HTTP_PORT", self.ports.candidate.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::{ComposeEnv, ComposePorts};
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
            ComposePorts {
                target: 28101,
                candidate: 28102,
            },
        );
        let keys = [
            "OMAKURE_ENROLLMENT_TARGET_TOKENS_FILE",
            "OMAKURE_ENROLLMENT_TARGET_CLIENT_FILE",
            "OMAKURE_ENROLLMENT_CANDIDATE_TOKENS_FILE",
            "OMAKURE_ENROLLMENT_CANDIDATE_CLIENT_FILE",
            "DIRECT_A_HTTP_PORT",
            "DIRECT_B_HTTP_PORT",
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
                    "28101",
                    "28102",
                ])
                .map(|(key, value)| (OsString::from(key), Some(OsString::from(value))))
                .collect::<BTreeMap<_, _>>()
        );
        assert_eq!(
            keys.iter().map(std::env::var_os).collect::<Vec<_>>(),
            inherited
        );
        assert_eq!(
            env.client_file("http://127.0.0.1:28102/v1/node/status"),
            PathBuf::from("/test/candidate.client")
        );
        assert_eq!(
            env.client_file("http://127.0.0.1:28101/v1/node/status"),
            PathBuf::from("/test/target.client")
        );
        assert_eq!(env.target_api(), "http://127.0.0.1:28101");
        assert_eq!(env.candidate_api(), "http://127.0.0.1:28102");
        let compose = include_str!("../../ci/compose/compose.direct-transport.e2e.yaml");
        assert!(compose.contains("127.0.0.1:${DIRECT_A_HTTP_PORT:-17878}:7878"));
        assert!(compose.contains("127.0.0.1:${DIRECT_B_HTTP_PORT:-17879}:7878"));
    }
}
