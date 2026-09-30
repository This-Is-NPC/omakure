use crate::auth::{AuthContext, Authenticator};
use crate::direct_service::TransportStatusHandle;
use crate::policy::DeployPolicy;
use crate::workspace::Workspace;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Shared readiness gate for `GET /v1/ready`.
///
/// Minimal by design: callers only learn whether the process is ready, never
/// token IDs, paths, or secret metadata.
#[derive(Debug)]
pub(crate) struct ReadinessGate {
    pub requires_worker: bool,
    pub requires_scheduler: bool,
    pub workers_configured: bool,
    pub scheduler_configured: bool,
    pub requires_transport: bool,
    pub transport_configured: bool,
    pub workers_alive: AtomicBool,
    pub scheduler_alive: AtomicBool,
    pub transport_alive: AtomicBool,
}

impl ReadinessGate {
    #[cfg(test)]
    pub(crate) fn new(
        requires_worker: bool,
        requires_scheduler: bool,
        workers_configured: bool,
        scheduler_configured: bool,
    ) -> Arc<Self> {
        Self::new_with_transport(
            requires_worker,
            requires_scheduler,
            workers_configured,
            scheduler_configured,
            false,
            false,
        )
    }

    pub(crate) fn new_with_transport(
        requires_worker: bool,
        requires_scheduler: bool,
        workers_configured: bool,
        scheduler_configured: bool,
        requires_transport: bool,
        transport_configured: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            requires_worker,
            requires_scheduler,
            workers_configured,
            scheduler_configured,
            requires_transport,
            transport_configured,
            workers_alive: AtomicBool::new(false),
            scheduler_alive: AtomicBool::new(false),
            transport_alive: AtomicBool::new(!transport_configured),
        })
    }

    pub(crate) fn is_ready(&self) -> bool {
        if self.requires_worker
            && self.workers_configured
            && !self.workers_alive.load(Ordering::SeqCst)
        {
            return false;
        }
        if self.requires_scheduler
            && self.scheduler_configured
            && !self.scheduler_alive.load(Ordering::SeqCst)
        {
            return false;
        }
        if self.requires_transport
            && self.transport_configured
            && !self.transport_alive.load(Ordering::SeqCst)
        {
            return false;
        }
        true
    }

    pub(crate) fn set_workers_alive(&self, alive: bool) {
        self.workers_alive.store(alive, Ordering::SeqCst);
    }

    pub(crate) fn set_scheduler_alive(&self, alive: bool) {
        self.scheduler_alive.store(alive, Ordering::SeqCst);
    }

    pub(crate) fn set_transport_alive(&self, alive: bool) {
        self.transport_alive.store(alive, Ordering::SeqCst);
    }
}

pub(super) struct ApiState {
    pub(super) auth: Authenticator,
    pub(super) workspace: Workspace,
    pub(super) policy: ApiPolicy,
    /// Deploy-time route-group gates (before scopes).
    pub(super) deploy: DeployPolicy,
    pub(super) readiness: Option<Arc<ReadinessGate>>,
    pub(super) transport: Option<TransportStatusHandle>,
    pub(super) discovery: Option<crate::discovery::DiscoveryStatusHandle>,
    /// Sends Cues over the sessions this process already holds.
    ///
    /// `None` when no direct transport is running, in which case there is
    /// nothing to dispatch over and the route says so rather than pretending.
    pub(super) cues: Option<crate::direct_service::CueDispatcher>,
    pub(super) baselines: Option<crate::direct_service::BaselineDispatcher>,
    pub(super) auth_verification_gate: Arc<tokio::sync::Semaphore>,
}

impl Clone for ApiState {
    fn clone(&self) -> Self {
        Self {
            auth: self.auth.clone(),
            workspace: self.workspace.clone_for_executor(),
            policy: self.policy.clone(),
            deploy: self.deploy.clone(),
            readiness: self.readiness.clone(),
            transport: self.transport.clone(),
            discovery: self.discovery.clone(),
            cues: self.cues.clone(),
            baselines: self.baselines.clone(),
            auth_verification_gate: Arc::clone(&self.auth_verification_gate),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApiCapability {
    ConfigRead,
    ScriptsRead,
    EnvRead,
    EnvWrite,
    EnvActivate,
    EnvUse,
    SecretProviderUse,
    SecretsReadMetadata,
    CredentialsUse,
    RunRead,
    BatteryRead,
    NodeRead,
    NodeWrite,
    EnrollmentRead,
    EnrollmentWrite,
    DiscoveryRead,
}

/// Secret-ref ACL from `--secret-ref`; an empty list denies provider refs.
#[derive(Debug, Clone, Default)]
pub(crate) struct ApiPolicy {
    allowed_secret_refs: Vec<String>,
}

impl ApiPolicy {
    pub(super) fn from_secret_refs(refs: &[String]) -> Self {
        Self {
            allowed_secret_refs: refs.iter().map(|r| r.trim().to_string()).collect(),
        }
    }

    #[cfg(test)]
    pub(super) fn with_secret_refs<const M: usize>(refs: [&str; M]) -> Self {
        Self {
            allowed_secret_refs: refs.into_iter().map(str::to_string).collect(),
        }
    }

    pub(super) fn secret_access(&self, auth: &AuthContext) -> crate::secrets::SecretAccess {
        let scopes = crate::secrets::SECRET_SCOPES
            .into_iter()
            .filter(|scope| auth.has_scope(scope))
            .collect();
        self.access_with_scopes(scopes)
    }

    /// Secret ACL for Battery HTTPS token_ref (requires credentials:use).
    pub(super) fn battery_credential_access(
        &self,
        auth: &AuthContext,
    ) -> crate::secrets::SecretAccess {
        if !auth.has_scope(crate::secrets::CREDENTIALS_USE_SCOPE) {
            return crate::secrets::SecretAccess::new(Vec::<&str>::new(), Vec::<String>::new());
        }
        self.access_with_scopes(vec![crate::secrets::CREDENTIALS_USE_SCOPE])
    }

    fn access_with_scopes(&self, scopes: Vec<&str>) -> crate::secrets::SecretAccess {
        let refs = &self.allowed_secret_refs;
        if refs.iter().any(|r| r == "*") {
            // Wildcard grants every file/provider ref but keeps env refs
            // gated behind explicitly listed `secret://env/...` entries.
            let env_refs = refs.iter().filter(|r| *r != "*").cloned();
            crate::secrets::SecretAccess::allow_all_non_env(scopes, env_refs)
        } else {
            crate::secrets::SecretAccess::new(scopes, refs.iter().cloned())
        }
    }
}

impl ApiCapability {
    pub(super) fn as_scope(&self) -> &'static str {
        match self {
            Self::ConfigRead => "config:read",
            Self::ScriptsRead => "scripts:read",
            Self::EnvRead => "envs:read",
            Self::EnvWrite => "envs:write",
            Self::EnvActivate => "envs:activate",
            Self::EnvUse => "envs:use",
            Self::SecretProviderUse => crate::secrets::SECRETS_USE_SCOPE,
            Self::SecretsReadMetadata => crate::secrets::SECRETS_READ_METADATA_SCOPE,
            Self::CredentialsUse => crate::secrets::CREDENTIALS_USE_SCOPE,
            Self::RunRead => "runs:read",
            Self::BatteryRead => "batteries:read",
            Self::NodeRead => "node:read",
            Self::NodeWrite => "node:write",
            Self::EnrollmentRead => "enrollment:read",
            Self::EnrollmentWrite => "enrollment:write",
            Self::DiscoveryRead => "discovery:read",
        }
    }
}
