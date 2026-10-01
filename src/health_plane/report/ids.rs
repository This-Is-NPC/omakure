use crate::util::hex;
use sha2::{Digest, Sha256};

/// Domain separator for the opaque Health Plane run identifier.
///
/// The shipped run id is `<unix_ms>-<pid>-<counter>`, which carries a host
/// process id and does not match the frozen 16-byte opaque form. Hashing it
/// under a dedicated domain yields a stable, opaque, P0 identifier that leaks
/// no host fact and still correlates two Pulses about the same run.
const RUN_ID_DOMAIN: &[u8] = b"omakure/health-run-id/v1\0";

/// Domain separator for the stable Signal idempotency key.
///
/// `signal_id` must be identical for every retransmission of one logical
/// Signal, including after a Performer restart, because the frozen contract
/// makes it the application idempotency key. Deriving it from the already
/// opaque run identifier under a dedicated domain gives exactly that, with no
/// extra durable state and no host fact.
const SIGNAL_ID_DOMAIN: &[u8] = b"omakure/health-signal-id/v1\0";

/// The stable `signal_id` of the `run-completed` Signal for one opaque run id.
///
/// It is a pure function of the run, so a retransmission, a reconnect, or a
/// Performer restart reproduces exactly the same idempotency key and a
/// Conductor can never store the same terminal run twice.
pub fn run_signal_id(run_id: &str) -> String {
    let digest = Sha256::digest(
        [
            SIGNAL_ID_DOMAIN,
            crate::health_plane::model::SignalKind::RunCompleted
                .wire()
                .as_bytes(),
            b"\0",
            run_id.as_bytes(),
        ]
        .concat(),
    );
    hex::encode(&digest[..16])
}

/// Map a shipped run id onto the frozen 16-byte opaque identifier.
pub fn opaque_run_id(run_id: &str) -> String {
    let digest = Sha256::digest([RUN_ID_DOMAIN, run_id.as_bytes()].concat());
    hex::encode(&digest[..16])
}
