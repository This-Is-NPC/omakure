//! Frozen attempt exhaustion, certified over one real, continuously connected
//! production Noise session against a packaged Performer.
//!
//! `.docs/health-plane-contract.md` freezes three retries per message with a
//! 1/2/4-second backoff after a 5-second acknowledgement timeout, and freezes
//! that an exhausted Profile is dropped rather than retried forever. Proving
//! that end to end needs a Conductor that stays connected and never
//! acknowledges, which is not controllable black-box against the production
//! binary. Under Compose it is: this harness *is* the Conductor for the
//! duration of the phase.
//!
//! It is the responder for the shipped `Noise_XX_25519_ChaChaPoly_SHA256`
//! handshake, accepts the production probe, acknowledges it exactly as the
//! shipped listener does, and then withholds every single `health_ack`. The
//! Performer on the other end is a real `node serve` container with its own
//! registry, its own reporter, and its own session machinery.
//!
//! Run with:
//! `cargo test --test docker_health_plane_exhaustion -- --ignored --nocapture`

use omakure::direct_service::RETRY_BACKOFF;
use omakure::direct_transport::{
    envelope_nonce, sign_ack, unix_seconds, verify_envelope, HandshakeRole, NoiseHandshake,
    TransportCertificate, ENVELOPE_KIND, MAX_FRAME_LENGTH,
};
use omakure::health_plane::bounds::{ACK_TIMEOUT_SECONDS, MAX_RETRIES, RETRY_BACKOFF_SECONDS};
use omakure::node::{NodeContext, NodePathOverrides, NodePlatform};
use omakure::node_identity::NodeIdentity;
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the harness waits for the Performer's container to dial in.
const ACCEPT_BUDGET: Duration = Duration::from_secs(120);
/// How long one frame read may block.
const FRAME_TIMEOUT: Duration = Duration::from_secs(15);

/// The frozen number of sends for one message: the first attempt plus the
/// frozen retry budget.
const EXPECTED_PROFILE_SENDS: usize = MAX_RETRIES as usize + 1;

/// How many strays keep the Performer redialing on the fast part of the
/// shipped ladder.
///
/// A connection that opens and sends nothing costs a *dial* attempt, never a
/// Health Plane ack retry. The dialer no longer retires on those, but each one
/// advances it one rung down `RETRY_BACKOFF`, and past the last rung the delay
/// doubles away toward a sixty-second ceiling.
const FAST_DIAL_ATTEMPTS: usize = RETRY_BACKOFF.len();

/// The observation window, derived from the frozen schedule rather than picked.
///
/// Each retry waits the frozen acknowledgement timeout plus its frozen backoff,
/// so the whole exhaustion sequence completes within the sum below. The window
/// adds one more acknowledgement timeout of slack so the harness can also
/// observe that no fifth Profile follows.
fn observation_window() -> Duration {
    let scheduled: i64 = (MAX_RETRIES + 1) * ACK_TIMEOUT_SECONDS
        + RETRY_BACKOFF_SECONDS.iter().sum::<i64>()
        + ACK_TIMEOUT_SECONDS;
    Duration::from_secs(scheduled as u64 + 15)
}

fn env(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("{name} must be set by .scripts/health-plane-certification.sh"))
}

fn node_material(state_dir: &Path) -> (NodeIdentity, [u8; 32], TransportCertificate) {
    // The config must live outside the state directory: the shipped node
    // context refuses overlapping paths.
    let config_path = state_dir.parent().unwrap_or(Path::new(".")).join(format!(
        "{}-harness-node.toml",
        state_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("harness")
    ));
    if !config_path.exists() {
        std::fs::write(&config_path, "version = 1\n").expect("write harness node config");
    }
    let context = NodeContext::resolve_for(
        NodePlatform::current(),
        NodePathOverrides::new(Some(state_dir.to_path_buf()), Some(config_path)),
        true,
        None,
        None,
        None,
    )
    .expect("resolve the harness node context");
    let identity = NodeIdentity::load_existing(&context).expect("load the harness node identity");
    // Not `try_into().expect(...)`: the `TryInto<[u8; 32]>` error value *is* the
    // Vec, so that spelling prints the raw private key into the panic message,
    // which the runner forwards to stderr and CI captures. Report the length.
    let raw_private = std::fs::read(context.transport_key_path()).expect("read transport key");
    let private: [u8; 32] = <[u8; 32]>::try_from(raw_private.as_slice())
        .unwrap_or_else(|_| panic!("transport key length: {} bytes", raw_private.len()));
    let certificate = TransportCertificate::from_bytes(
        &std::fs::read(context.transport_certificate_path()).expect("read transport certificate"),
    )
    .expect("parse transport certificate");
    (identity, private, certificate)
}

fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    read_frame_detailed(stream).ok()
}

/// The same read, but surfacing the underlying I/O error. A bare `None` on the
/// first handshake frame is indistinguishable between "the peer sent nothing
/// and hung up" and "the peer truncated a frame", which is exactly the
/// distinction needed to tell a stray connection from a real protocol fault.
fn read_frame_detailed(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    // The prefix is unauthenticated at this point, so bound the allocation the
    // way the shipped reader does rather than trusting it.
    if !(4..=MAX_FRAME_LENGTH).contains(&length) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame length {length} is outside the shipped bound"),
        ));
    }
    let mut encoded = vec![0_u8; length + 4];
    encoded[..4].copy_from_slice(&prefix);
    stream.read_exact(&mut encoded[4..])?;
    Ok(encoded)
}

#[test]
#[ignore = "requires the live topology started by .scripts/health-plane-certification.sh"]
fn an_unacknowledged_profile_stops_at_the_frozen_attempt_budget_on_one_session() {
    let bind = env("OMAKURE_HP_EXHAUSTION_BIND");
    let state_dir = PathBuf::from(env("OMAKURE_HP_ADVERSARY_STATE"));
    let listener_id = env("OMAKURE_HP_ADVERSARY_ID");
    let performer_id = env("OMAKURE_HP_PERFORMER_ID");
    let (identity, private, certificate) = node_material(&state_dir);
    assert_eq!(
        identity.public_status().node_id,
        listener_id,
        "the harness must listen with the identity the Performer trusts as its Conductor"
    );

    let listener = TcpListener::bind(&bind).expect("bind the certification listener");
    listener
        .set_nonblocking(true)
        .expect("set the listener non-blocking");
    // Publish readiness only once the socket is accepting. The shipped dialer
    // stops after its frozen three attempts, so the Performer must not be
    // started before this point.
    std::fs::write(env("OMAKURE_HP_EXHAUSTION_READY"), "ready\n")
        .expect("publish the certification listener readiness marker");

    // Bounded accept: the Performer's container must dial in, or the phase
    // fails rather than waiting forever.
    //
    // A connection that opens and hangs up without sending a first handshake
    // frame is not the Performer's Noise dial. On this network the repointed
    // Performer is the only peer configured toward the harness, so a stray *is*
    // the shipped dialer opening a socket and abandoning it before its first
    // write - `connect_and_hold` has five fallible steps between the TCP
    // connect and that write, and any of them drops the stream silently.
    //
    // Accepting past it preserves the measurement, which is taken on whichever
    // session does handshake. The count is bounded below rather than required
    // to be zero, because a stray spends a dial attempt, not an ack retry.
    let deadline = Instant::now() + ACCEPT_BUDGET;
    let mut strays = 0_u32;
    let (mut stream, frame) = loop {
        assert!(
            Instant::now() < deadline,
            "the repointed Performer never completed a handshake with the harness \
             within {ACCEPT_BUDGET:?} ({strays} connection(s) opened and sent no frame)"
        );
        let mut stream = match listener.accept() {
            Ok((stream, peer)) => {
                eprintln!("harness: accepted a connection from {peer}");
                stream
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(error) => panic!("accepting the Performer connection failed: {error}"),
        };
        stream.set_nonblocking(false).expect("blocking stream");
        stream
            .set_read_timeout(Some(FRAME_TIMEOUT))
            .expect("read timeout");
        stream
            .set_write_timeout(Some(FRAME_TIMEOUT))
            .expect("write timeout");
        match read_frame_detailed(&mut stream) {
            Ok(frame) => break (stream, frame),
            Err(error) => {
                strays += 1;
                eprintln!("harness: discarding a connection that sent no handshake: {error}");
            }
        }
    };

    // Strays threaten this phase's setup rather than its measurement: the send
    // count asserted below is taken on the session that did handshake. Nothing
    // about reaching this line bounds how many came first, because the dialer
    // survives all of them, so this is a real bound and not a restatement of
    // the loop above.
    //
    // Beyond the fast rungs the delay doubles toward a minute, which would run
    // this phase past `ACCEPT_BUDGET` and report a timeout instead of the stray
    // storm that caused it.
    assert!(
        (strays as usize) < FAST_DIAL_ATTEMPTS,
        "the Performer opened {strays} connection(s) that sent no handshake frame, \
         pushing its redial past the {FAST_DIAL_ATTEMPTS} fast attempts and into \
         delays long enough to time this phase out for an unrelated-looking reason"
    );

    // The shipped responder path, verbatim: three handshake messages, then the
    // production probe and its acknowledgement.
    let mut handshake = NoiseHandshake::new(HandshakeRole::Responder, private, certificate)
        .expect("build the production Noise handshake");
    handshake
        .read_next(&frame, unix_seconds())
        .expect("read handshake message 1");
    stream
        .write_all(&handshake.write_next().expect("handshake message 2"))
        .expect("send handshake message 2");
    let frame = read_frame(&mut stream).expect("read handshake message 3");
    handshake
        .read_next(&frame, unix_seconds())
        .expect("read handshake message 3");
    let remote = handshake
        .remote_certificate()
        .cloned()
        .expect("the Performer must present a transport certificate");
    assert_eq!(
        remote.node_id(),
        performer_id,
        "an unexpected node dialled the certification listener"
    );
    let mut session = handshake.into_session().expect("establish the session");

    let frame = read_frame(&mut stream).expect("read the production probe");
    let probe = session.read(&frame).expect("decrypt the probe");
    assert_eq!(
        probe.kind, ENVELOPE_KIND,
        "the Performer must send an envelope"
    );
    let nonce = envelope_nonce(&probe.body).expect("read the probe nonce");
    verify_envelope(
        &probe.body,
        remote.node_id(),
        remote.identity_key(),
        "probe",
        session.session_id(),
        &nonce,
    )
    .expect("the Performer's probe must verify under the frozen construction");
    let ack = sign_ack(&identity, session.session_id(), nonce, unix_seconds())
        .expect("sign the probe acknowledgement");
    let encoded = session
        .write(ENVELOPE_KIND, &ack.encoded())
        .expect("encrypt the probe acknowledgement");
    stream
        .write_all(&encoded)
        .expect("send the probe acknowledgement");

    // From here the harness is a Conductor that never acknowledges anything.
    // The session stays open for the whole window, so every Profile the
    // Performer sends is a retry on *this* session rather than a resend on a
    // later one.
    let window = observation_window();
    let deadline = Instant::now() + window;
    let mut profile_ids: Vec<String> = Vec::new();
    let mut pulse_ids: BTreeSet<String> = BTreeSet::new();
    let mut last_profile_at: Option<Instant> = None;
    let mut first_profile_at: Option<Instant> = None;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        stream
            .set_read_timeout(Some(remaining.max(Duration::from_millis(100))))
            .expect("observation timeout");
        let Some(frame) = read_frame(&mut stream) else {
            break;
        };
        let message = session
            .read(&frame)
            .expect("decrypt the Health Plane envelope");
        let value: Value = serde_json::from_slice(&message.body[..message.body.len() - 64])
            .expect("parse the Health Plane envelope");
        let kind = value["kind"].as_str().unwrap_or_default().to_string();
        let id = value["payload"]["message_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // Every attempt must be a *fresh* message, because the frozen replay
        // and freshness rules reject a byte-identical resend.
        match kind.as_str() {
            "health_profile" => {
                assert!(
                    !profile_ids.contains(&id),
                    "a retry reused message_id {id}, which the frozen replay rule forbids"
                );
                if first_profile_at.is_none() {
                    first_profile_at = Some(Instant::now());
                }
                last_profile_at = Some(Instant::now());
                profile_ids.push(id);
            }
            "health_pulse" => {
                pulse_ids.insert(id);
            }
            other => panic!("the Performer sent an unexpected Health Plane kind {other}"),
        }
    }

    assert_eq!(
        profile_ids.len(),
        EXPECTED_PROFILE_SENDS,
        "an unacknowledged Profile must be sent exactly {EXPECTED_PROFILE_SENDS} times \
         (one attempt plus the frozen {MAX_RETRIES} retries) and then dropped; observed {:?}",
        profile_ids.len()
    );
    let first = first_profile_at.expect("at least one Profile");
    let last = last_profile_at.expect("at least one Profile");
    let spent = last.duration_since(first);
    let ceiling = Duration::from_secs(
        (MAX_RETRIES * ACK_TIMEOUT_SECONDS + RETRY_BACKOFF_SECONDS.iter().sum::<i64>()) as u64 + 10,
    );
    assert!(
        spent <= ceiling,
        "the frozen retry schedule took {spent:?}, beyond its own bound of {ceiling:?}"
    );
    assert!(
        !pulse_ids.is_empty(),
        "after the Profile budget was spent the Performer must move on to its Pulse schedule \
         on the same session, which proves the session stayed connected throughout"
    );
}
