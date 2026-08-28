# Direct Transport and Enrollment Contract

**Status: IMPLEMENTED CONTRACT.** This document freezes the direct transport
and enrollment protocol implemented by the production node listener, discovery
service, and node registry. It remains the compatibility and review contract;
dependent work must use these bytes and state transitions without inventing a
second wire format.

## Decision

The channel uses the standard Noise `XX_25519_ChaChaPoly_SHA256` construction
through `snow` `0.10.0`, with its default pure-Rust resolver and only the
`use-curve25519`, `use-chacha20poly1305`, `use-sha2`, and `use-getrandom`
features. The standard construction supplies ephemeral and static X25519 key
agreement, ChaCha20-Poly1305 transport encryption, and the Noise SHA-256 HKDF
key schedule. No custom Noise resolver, secp256k1 DH adapter, cipher, KDF, or
signature protocol is permitted.

The existing normalized secp256k1/BIP-340 key remains the canonical machine
and application identity. Each service additionally owns one static X25519
transport key. The X25519 key is channel material only; it never becomes a
`node_id`, signs an application envelope, or creates trust. A signed transport
certificate binds the two materials before the channel is accepted.

The selected construction is covered by protocol fixtures, direct transport
integration tests, and the bounded Linux multi-node certification. Cross-platform
native protocol/build/lifecycle coverage remains in the hosted CI matrix.

## Version and Domains

The protocol version is `1`. All integers below are unsigned big-endian unless
another encoding is stated. All text is UTF-8 and must be valid, shortest-form
UTF-8 with no NUL or control characters. Unencrypted outer-frame versions,
lengths, kinds, and flags are rejected before cryptographic work. Fields inside
Noise ciphertext are checked only after decryption and authentication, before
certificate validation, trust, or application delivery. A certificate payload
kind is therefore a post-crypto/pre-trust check, not a pre-crypto claim.

| Name | Exact bytes |
|---|---|
| Contract ID | ASCII `omakure/direct-transport/v1` |
| Noise protocol name | ASCII `Noise_XX_25519_ChaChaPoly_SHA256` |
| Noise prologue | ASCII `omakure/direct-transport/v1` followed by one NUL byte |
| Certificate signature domain | ASCII `omakure/transport-cert/v1` followed by one NUL byte |
| Enrollment bundle signature domain | ASCII `omakure/enrollment-bundle/v1` followed by one NUL byte |
| Direct envelope signature domain | Existing `omakure/direct-envelope/v1` followed by one NUL byte |
| Node ID domain | Existing `omakure/node-id/v1` followed by one NUL byte |

The Noise prologue is hashed into the Noise handshake hash by both parties. A
prologue mismatch causes authentication failure; it is never negotiated.

## LAN Discovery Beacon

LAN discovery is a locator only. It is not a transport handshake, an enrollment
exchange, a trust decision, or an authorization mechanism. A received Beacon may
produce an in-memory candidate observation; it MUST NOT create a registry row,
transport session, pending enrollment, or active peer. The direct handshake and
the existing trust/enrollment policy remain required before any useful direct
communication.

The Beacon contract is version `1` and uses these fixed values:

| Name | Exact bytes |
|---|---|
| Beacon magic | ASCII `OMKB` |
| Beacon contract ID | ASCII `omakure/lan-discovery/v1` |
| Beacon signature domain | ASCII `omakure/lan-beacon/v1` followed by one NUL byte |
| IPv4 multicast group | `239.255.42.99` |
| UDP port | `38383` |

All Beacon integers are unsigned big-endian. The complete datagram is at most
`247` bytes with a discovery proof and `215` bytes without one. Receivers MUST
read into a `512`-byte bounded buffer, reject a datagram larger than `512` bytes,
and never allocate from an unauthenticated length field. The unsigned Beacon
body is:

```text
OMKB:u32 || version:u8(1) || kind:u8(1) || flags:u16 ||
node_id:69 || identity_xonly:32 || beacon_id:16 || direct_port:u16 ||
issued_at:u64 || expires_at:u64 || sequence:u64 ||
[discovery_proof:32 when flags bit 0 is set] ||
signature:64
```

`flags` is zero or `1`; all other bits reject. `node_id` is the canonical
lowercase `omk1_` value derived from `identity_xonly`. `beacon_id` is a random
opaque process-instance identifier. `direct_port` is the node's direct TCP
listener port; the source IP address of the UDP datagram, not any Beacon field,
is the candidate address. `sequence` increases for each Beacon sent by that
process. A Beacon is valid when `issued_at <= now < expires_at`, is no more than
5 seconds in the future, and has a lifetime of at most 15 seconds.

The signature is BIP-340 over:

```text
SHA-256(ASCII("omakure/lan-beacon/v1"), NUL, all Beacon bytes before signature)
```

When flags bit 0 is set, `discovery_proof` is:

```text
HMAC-SHA256(discovery_secret,
  ASCII("omakure/lan-discovery-proof/v1"), NUL,
  all Beacon bytes from OMKB through sequence)
```

The secret is an optional process-only value resolved from the configured
`secret://provider/name` by the node's secret provider. It is never written to
node config, SQLite, logs, status, HTTP responses, or Beacon error text. If a
local discovery secret is configured, unproved or mismatched Beacons are
discarded. Without a local secret, unproved Beacons are accepted and proved
Beacons are discarded because they cannot be verified. Proof success is only
evidence that the sender knows the same secret; it never authorizes the sender.

The sender transmits every 3 seconds to the IPv4 multicast group and, when
enabled by the node configuration, the IPv4 limited broadcast address and each
enumerated interface broadcast address. The receiver joins multicast on every
usable local IPv4 interface. Failure to enumerate or join one interface does
not disable the others. A platform without a usable UDP/multicast facility
reports discovery as unsupported and does not claim that discovery is active.

The receiver enforces these protocol bounds before identity work:

| Resource | Limit |
|---|---:|
| Datagram buffer | 512 bytes |
| Global accepted/parsed datagrams | 64 per second |
| Source-IP datagrams | 8 per second |
| Source-IP table | 256 entries |
| Candidate observations | 256 total |
| Addresses per node ID | 8 |
| Candidate retention | Until `expires_at`, then immediate expiry |
| Send interval | 3 seconds |
| Beacon lifetime | 15 seconds |
| Future clock skew | 5 seconds |

Rate counters and candidates are pruned on every receive pass and at least every
second. Duplicate `(node_id, source-IP, direct_port)` observations update the
existing bounded record only when the Beacon is newer by `(issued_at, sequence)`.
When the candidate bound is full, a new key is dropped; it never evicts a live
candidate in response to unauthenticated input. Expired records may be removed
to make room. No response is sent to malformed, spoofed, stale, secret-mismatch,
or rate-limited input.

Stable discovery errors are `unsupported_version`, `invalid_beacon`,
`message_too_large`, `expired`, `future`, `secret_mismatch`,
`identity_mismatch`, `signature_invalid`, `rate_limited`, `candidate_limit`,
`unsupported_platform`, and `internal`. They reveal no raw datagram, secret,
signature, or private interface data.

Discovery status and candidates are exposed only through bounded shared
operations. Default status includes enabled/listening/support state, counts,
limits, and redacted candidate identity/time evidence. An explicit
`discovery:read` authorization scope may request observed source addresses;
the default status and `node:read` status never return private interface
topology. The CLI and authenticated HTTP surfaces use the same operation and
scope rules. Shutdown closes the UDP socket and joins its sender/receiver
thread; restart starts with an empty in-memory observation set.

## Identity and Transport Keys

The identity rules this contract binds:

- `identity.key` contains exactly one normalized 32-byte big-endian
  secp256k1 scalar. It is normalized to the even-Y representative before
  persistence, rotation, recovery, and signing.
- The public identity is the 32-byte lowercase-hex BIP-340 x-only public key.
- `node_id` is `omk1_` plus lowercase hex of
  `SHA-256(ASCII("omakure/node-id/v1"), NUL, x_only_public_key)`.
- BIP-340 Schnorr is the only node/application signature algorithm.
- A node ID, discovery result, relay, successful Noise handshake, or presented
  certificate cannot create active trust.

The transport key is stored separately as machine-owned private state under
`.omakure/` with permissions equivalent to `identity.key`. Its public value is
32 raw X25519 bytes. Production generation uses OS CSPRNG output. Import,
rotation, backup, and deletion are atomic and fail closed; a missing or
malformed transport key prevents direct mode from starting rather than
generating a replacement silently. The transport key has an independent
monotonic `key_epoch`; rotating it requires a new certificate and an explicit
authorized peer update.

## Transport Certificate

The certificate is a fixed 245-byte binary record. The signed body is the
first 181 bytes; the last 64 bytes are the BIP-340 signature over
`SHA-256(certificate_signature_domain || signed_body)`.

| Offset | Length | Field | Rule |
|---:|---:|---|---|
| 0 | 4 | magic | ASCII `OMTC` |
| 4 | 1 | version | `1` |
| 5 | 1 | kind | `1` for transport binding |
| 6 | 2 | flags | `0`; all other values reject |
| 8 | 32 | x-only identity key | Canonical BIP-340 public key |
| 40 | 69 | node ID | ASCII `omk1_` plus 64 lowercase hex chars |
| 109 | 32 | X25519 static public key | Raw little-endian X25519 public value |
| 141 | 8 | key epoch | Non-zero, increasing per identity |
| 149 | 8 | not-before | Unix seconds |
| 157 | 8 | not-after | Unix seconds, greater than not-before |
| 165 | 16 | certificate ID | Random opaque ID, stable for this certificate epoch |
| 181 | 64 | BIP-340 signature | Signature over bytes `0..181` and the certificate domain |

The receiver checks the magic, version, reserved flags, exact length, valid
X25519 encoding, identity-key/node-ID derivation, signature, epoch, validity
window, and local revocation before accepting the certificate. `not-after -
not-before` is at most 63,072,000 seconds (two 365-day years), `key_epoch` is
non-zero and strictly greater than the last retained epoch, and the transport
public key must pass the complete low-order/all-zero X25519 check. The X25519
key in the certificate must equal Noise `get_remote_static()` after the Noise
XX message that reveals the remote static key. A valid certificate with an
unknown or inactive identity remains untrusted.

The certificate is carried as a Noise handshake payload with no re-encoding:
`payload = kind:u8(1) || certificate:245 bytes`. Message 1 has an empty
payload. Message 2 carries the responder payload and Message 3 carries the
initiator payload. A missing kind byte, any kind other than `1`, or any payload
length other than `246` is rejected after Noise authentication and before the
certificate is parsed or the session is authorized.

## Noise Handshake

Both parties construct `snow::Builder` with the exact protocol name and
prologue, load their local transport private key, and do not configure a
remote static key in advance. The XX pattern is therefore:

| Message | Noise tokens | Payload |
|---|---|---|
| 1, initiator to responder | `e` | Empty |
| 2, responder to initiator | `e, ee, s, es` | Responder certificate |
| 3, initiator to responder | `s, se` | Initiator certificate |

Each Noise message is sent in one protocol frame. Noise ciphertext bytes are
not base64, JSON, compressed, or otherwise transformed. The receiver passes
the exact message bytes to `read_message`, verifies the kind byte and payload
length, verifies the certificate,
then checks that the certificate transport key equals the Noise remote static
key. The channel is not established until both certificates pass and the
handshake state reports completion.

The reference adapter owns this entire receive path: it validates the frame
length and kind, enforces the `1 -> 2 -> 3` handshake order, performs the
pre-read and post-decryption X25519 checks, calls `snow::read_message` with the
unchanged bytes, parses the certificate payload, and checks the certificate's
transport key against `get_remote_static()`. No test may authorize a session
by calling an individual parser or Snow method outside that adapter path.

### X25519 integration boundary

`snow` `0.10.0` does not reject the complete X25519 low-order encoding set
itself. The integration MUST wrap every `read_message` and every local key
installation with this boundary:

1. Before passing Noise message 1 or 2 to `snow`, parse the first 32 bytes as
   the remote ephemeral public key and pass it through the adapter. The adapter
   rejects a non-32-byte value and every canonical encoding in this complete
   seven-entry blacklist: `0000000000000000000000000000000000000000000000000000000000000000`,
   `0100000000000000000000000000000000000000000000000000000000000000`,
   `e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800`,
   `5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157`,
   `ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f`,
   `edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f`, and
   `eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f`. It also rejects any
   non-blacklisted encoding whose RFC 7748 probe result is all zero.
2. Immediately after message 2 or 3 is decrypted, obtain the remote static
   key from `get_remote_static()`, pass it through the same adapter, and reject
   before certificate validation, `into_transport_mode`, or any authorization
   decision. This post-`snow` check is intentional: XX encrypts the remote
   static key, so it cannot be inspected before Noise decrypts it.
3. Before installing a local static key, derive its public key with RFC 7748,
   verify the 32-byte encoding, and verify the probe result is not all zero.
   `snow` does not expose its locally generated ephemeral private/public pair,
   but that is not needed for this boundary: Noise-generated private scalars
   are clamped by the resolver, and every attacker-controlled remote ephemeral
   or static public value is checked with a clamped local probe before the
   corresponding DH result can authorize a session. If a future resolver does
   not guarantee RFC 7748 clamping, the contract becomes infeasible and
   production transport must stop rather than claim low-order protection.

The blacklist comparison is constant-time and the probe uses the constant-time
RFC 7748 operation with an OR-of-bytes zero test. No DH result is accepted as a
Noise/session authorization input until the boundary passes. The public
fixture contains every one of the seven prohibited encodings plus a valid
RFC 7748 vector; the adapter test feeds each prohibited encoding through the
same check and asserts rejection, never successful all-zero DH.

The final Noise handshake hash is the session binding. The session ID is the
full 32-byte final handshake hash; it is included in every encrypted transport
frame. The two sides must derive the same hash or abort. No application field
is substituted for the Noise hash.

Noise supplies the handshake HKDF and directional cipher states. There is no
additional application KDF. Noise transport nonce counters start at zero for
each direction and increment exactly once per successfully written or read
Noise transport message. A counter mismatch, duplicate, gap, or overflow
aborts the session. The implementation tracks the counters outside `snow`; the
encrypted inner sequence is checked immediately after successful decryption and
before control handling or application delivery.

The reference adapter performs outer-frame parsing, session-ID comparison,
Noise transport decryption, inner-kind parsing, and sequence authorization as
one receive operation. `authenticated_untrusted` may stage only the explicitly
enumerated `manual_request` and `bundle` enrollment messages. It cannot accept
an application envelope, Close, Error, or any other control sequence. Only an
`active` identity may advance application or control sequences, and every
rotation/revocation transition requires explicit signed authority evidence or
the local operator transaction.

The implementation calls the standard `snow` rekey operation before the frame
whose next direction sequence reaches either 1,048,576 messages or 1 GiB of
plaintext, whichever comes first. That frame is encrypted under the new key;
the preceding frame is the last old-key frame. Rekey does not reset the frame
sequence. Both peers independently apply `rekey_outgoing` before their own
boundary frame and `rekey_incoming` before reading the peer's boundary frame.
The event is deterministic from direction-local counters, so both directions
are synchronized without a new wire message. Missing or early rekey causes
authentication failure and closes the session. A session closes before nonce
counter exhaustion; it never wraps a nonce. There is no session resumption or
0-RTT data. A failed write does not advance sequence, byte, or rekey counters;
it increments the session `write_failures` counter and closes after 3 failures.

## Framing and Data

The channel requires a reliable ordered byte stream. Each outer frame is:

```text
length:u32be || version:u8 || kind:u8 || flags:u16be || body:(length - 4) bytes
```

`length` counts `version`, `kind`, `flags`, and `body`, but not the four length
bytes. It must be at least `4` and at most `1,048,580`; the complete frame is
therefore at most `1,048,584` bytes. A receiver reads exactly the declared
length, rejects truncation, and never allocates based on an unchecked value.
Flags must be zero in version 1.

| Kind | Value | Body |
|---|---:|---|
| Handshake | 1 | `message_number:u8` (`1..=3`) || exact Noise message bytes |
| Encrypted transport | 2 | `session_id:32` || Noise transport ciphertext |

Handshake bodies are limited to 4,096 bytes and only messages 1, 2, and 3 are
accepted in order. The encrypted transport plaintext is:

```text
sequence:u64be || inner_kind:u8 || inner_version:u8 || inner_body
```

`inner_version` is `1`. `inner_kind=1` is a direct envelope and its body is
the exact canonical envelope bytes plus its 64-byte BIP-340 signature.
`inner_kind=2` is Close with `reason:u16be`; `inner_kind=3` is Error with
`error_code:u16be`. No Close or Error value exists in the outer frame kind.
The sequence is encrypted and authenticated by Noise, and must be exactly the
next direction-local sequence. Data plaintext is limited to 1,048,520 bytes
before Noise encryption so the session, cipher-tag, inner sequence, and frame
headers fit within the complete 1,048,584-byte frame limit. The direct envelope
canonical serialization and BIP-340 signature are:

```text
SHA-256(ASCII("omakure/direct-envelope/v1"), NUL,
        strict RFC-8785 canonical UTF-8 envelope-without-signature)
```

The outer frame is transport demultiplexing metadata and is not included in the
direct envelope signature. Encrypted transport received before handshake
completion, with a wrong session ID, wrong direction sequence, invalid
ciphertext, invalid inner control, or invalid envelope is rejected without
delivery or retry. Close and Error are only generated after authenticated
decryption and sequence validation.

## Version, Downgrade, and Errors

Version 1 has no algorithm negotiation. The Noise name, prologue, frame
version, certificate version, and enrollment version are fixed constants. A
future version must use a new contract ID and domain separator. It cannot be
selected by removing fields from version 1 or by silently falling back to an
older version.

Stable protocol error codes are:

| Code | Name | Meaning |
|---:|---|---|
| 1001 | `unsupported_version` | Unsupported frame, certificate, or enrollment version |
| 1002 | `invalid_frame` | Length, kind, flags, ordering, or encoding failure |
| 1003 | `message_too_large` | A frame, handshake, or plaintext exceeds its limit |
| 1004 | `handshake_failed` | Noise state, prologue, certificate, or static-key failure |
| 1005 | `identity_mismatch` | Certificate key and node ID do not pair |
| 1006 | `not_enrolled` | Identity is not an active trusted peer |
| 1007 | `revoked` | Identity, certificate epoch, or bundle is revoked |
| 1008 | `expired` | Certificate or enrollment is outside its validity window |
| 1009 | `replay` | Bundle ID, request ID, or duplicate established-data sequence was replayed |
| 1010 | `rate_limited` | Peer/session/work limit was reached |
| 1011 | `internal` | Bounded local failure; no trust mutation occurred |

Errors reveal only the stable code and no key, signature, plaintext, or raw
frame. A malformed unauthenticated peer is closed without an error response
unless the frame can be parsed safely.

Retry behavior turns on what failed. A handshake refused on its merits is not
retried at all: `unsupported_version`, `invalid_frame`, `message_too_large`,
`handshake_failed`, `identity_mismatch`, `not_enrolled`, `revoked`, `expired`,
and `replay` stop a static-peer dialer for the life of the process, and
recovery requires a new operator or caller action. A peer that is merely
unreachable is a different failure and is retried without limit, because
nothing respawns a dialer that gives up and a peer down for a few seconds at
the wrong moment would otherwise be lost until the process restarted. Those
attempts wait 1 s, then 2 s, then 4 s, then keep doubling to a 60-second
ceiling, each delay carrying up to 250 ms of jitter so a fleet restarting
together does not resynchronise. The ceiling is bounded rather than chosen: one
whole delay plus its jitter plus the connect and handshake budgets must still
leave a peer that comes back inside the Health Plane's Online window.

## Time, Replay, and Resource Limits

The limits are protocol constants, not inherited implicitly from the HTTP
surface:

| Resource | Limit |
|---|---:|
| Certificate validity future skew | 300 seconds |
| Certificate validity maximum lifetime | 2 years |
| Bundle validity future skew | 300 seconds |
| Bundle maximum lifetime | 30 days |
| Handshake wall clock | 10 seconds |
| Idle established session | 300 seconds |
| Concurrent peers | 1,024 |
| Concurrent sessions per active peer | 4 |
| Unauthenticated handshakes per source | 4 per minute |
| Queued frames per session | 64 |
| Frame bytes | 1,048,584 including length prefix |
| Handshake message bytes | 4,096 |
| Decrypted plaintext bytes | 1,048,520 |
| Capabilities in a bundle | 32, sorted and unique |
| Capability bytes | 64 each |
| Enrollment replay retention | Through expiry plus 24 hours |
| Handshake retries after a refusal | 0; the dialer stops |
| Reconnect attempts for an unreachable peer | Unbounded |
| Reconnect backoff | 1 s, 2 s, 4 s, doubling to a 60 s ceiling, plus jitter |
<!-- The three rows above replaced a single frozen `Handshake retries | 3`.
     That row was false in both directions: a handshake refused on its merits
     was never retried at all — nine of the eleven transport errors are fatal —
     and the three attempts it named governed reachability, where they made a
     peer unreachable for three seconds unrecoverable until restart. The
     correction of the description needs no approval; the change from three
     attempts to unbounded-with-a-ceiling is a behaviour change to a frozen
     number and is **recorded here for owner review**. -->
| Noise rekey trigger | 1,048,576 messages or 1 GiB per direction |

The listener also enforces 256 global in-flight handshakes, 4 new handshakes
per minute per rate key, 64 MiB of global queued frame bytes, 4 MiB per source
rate key, and 256 KiB per session. Before authentication the rate key is the
canonical source IP; after a certificate is structurally authenticated it is
`source-IP || node_id`. A source key is never treated as identity or trust.
These budgets bound work arriving from a stranger, so a dial this node makes
itself takes none of them; it is charged only against the global ceilings and,
once its peer's certificate authenticates, against that identity. Otherwise a
node sharing an address with its peers would spend their inbound allowance on
its own outgoing links.
The four-byte header has a 2-second read deadline. The body deadline is
1 second plus 1 second per started 64 KiB, capped at 10 seconds; the handshake
wall clock remains 10 seconds. Each malformed input consumes one of four
parser/work units per connection; the fifth closes the connection. Cleanup
runs every 30 seconds, closes partial handshakes past deadline, expires idle
sessions, releases all queue-byte reservations, and deletes only replay rows
past their retention floor. No cleanup operation scans or allocates an
unbounded input buffer.

Clocks use UTC Unix seconds. A validity boundary is inclusive at not-before
and exclusive after not-after. Clock skew beyond 300 seconds fails closed.
Bundle IDs, manual request IDs, and direct Cue IDs are stored before work is
authorized and retained through their expiry window plus 24 hours. Certificate
IDs are not stored in replay keys. A duplicate bundle, request, or data
sequence produces `replay` and cannot execute application work a second time.

## Enrollment Authority

Enrollment is separate from channel authentication. A completed Noise channel
never changes trust. `enrollment = "disabled"` remains the safe default.

The role enum is exact: `1 = conductor`, `2 = performer`; zero and every other
value reject. A capability is 1 to 64 ASCII bytes matching
`[a-z0-9][a-z0-9._-]{0,63}` and must be one of the configured allow-list:
`backup-orchestration`, `baseline-push`, `inventory-health`,
`lost-device-revocation`, `notifications`, `remote-run`, or
`ssh-credential-rotation`. Capabilities are sorted by raw byte order and
deduplicated before encoding. A bundle or manual request contains at most 32
capabilities and at most 4,096 bytes of capability encoding; duplicate,
unsorted, unsupported, empty, or oversized values reject.

### Manual enrollment

Manual pairing uses two direction-specific signed OMMA requests. A single
16-byte `pairing_id` links the requests: node A stages node B's request and
node B stages node A's request. Each local operator approves only the exact
request proposed by the remote node. A one-sided approval activates only that
direction's remote identity, trusted peer, and transport epoch; it cannot
authorize application traffic in the opposite direction. Bidirectional
application traffic is authorized only after both local approvals complete.

The request record is the following fixed binary value:

```text
OMMA:u32 || version:u8(2) || pairing_id:16 || request_id:16 || proposer_node_id:69 ||
proposer_xonly:32 || proposer_transport_x25519:32 || role:u8 ||
capability_count:u8 || repeated(capability_len:u16 || capability_utf8) ||
created_at:u64 || expires_at:u64 || code_hash:32 || proposer_bip340_signature:64
```

`OMMA` is ASCII and version `2` is the only accepted manual request version.
The request is limited to 2,048 bytes; capabilities are sorted and unique.
`pairing_id` is random, non-zero, and identical in both direction-specific
requests. It is included in the signed body and is not itself a replay key.
The displayed approval code is 16 random bytes encoded as
32 lowercase hexadecimal characters. `code_hash` is
`SHA-256(ASCII("omakure/manual-enrollment/v1"), NUL, code_bytes)`. The code
is an out-of-band confirmation value, not a bearer credential and not proof of
identity. The proposer signature covers every byte through `code_hash`,
preceded by the same manual-enrollment domain. The receiver derives the
proposer identity key from the request's x-only field, derives and checks its
`node_id`, and verifies this signature before staging. A node rejects a request
proposed by itself. Each operator confirms the displayed pairing ID, remote node
ID, x-only key, role, capabilities, expiry, and code locally. Each approval
atomically activates only the remote request's direction; the first completed
approval for each direction wins.

The executable public vector uses pairing ID
`00000000000000000000000000000002`, request ID
`00000000000000000000000000000001`, code bytes
`000102030405060708090a0b0c0d0e0f`, and code hash
`e9380fb38041d9a4cb70fbca9631da6d796fea839738fbd6e7015d829ccd54f7`.
The reference verifier parses and checks every field, exact total length,
version, request ID, node/key/transport bindings, role, capability lengths and
policy, validity ordering and maximum lifetime, recomputed code hash, and the
signature. Mutating any one of those fields, or appending bytes, must reject.

### Signed bundle enrollment

The offline bootstrap authority or an already trusted manager signs the exact
bundle body below. Length-prefixed text fields use `u16be`; the maximum bundle
size is 8,192 bytes.

```text
OMEB:u32 || version:u8(1) || flags:u16(0) || bundle_id:16 ||
authority_key_id:16 || organization_len:u16 || organization_utf8 ||
audience_node_id:69 || subject_node_id:69 || subject_xonly:32 ||
subject_transport_x25519:32 || subject_certificate:245 || role:u8 || capability_count:u8 ||
repeated(capability_len:u16 || capability_utf8) || issued_at:u64 ||
expires_at:u64 || authority_bip340_signature:64
```

The signature covers every byte through `expires_at`, including the complete
245-byte subject certificate, preceded by the enrollment bundle domain. The
receiver selects the authority public key from local configuration by
`authority_key_id`; a public key carried in the bundle cannot introduce an
authority. It checks organization, audience, subject key/ID pairing, the
complete certificate signature and transport-key pairing, role, sorted
capability policy, signature, expiry/skew, bundle replay ID, and local revocation before the
single transaction that inserts the peer as pending or active according to the
configured policy. A manager cannot enroll itself.

The first valid completion wins. A second manager, role change, or replacement
requires an authenticated authorized update with a new bundle ID. Discovery,
relay delivery, copied database state, TOFU, node IDs, and successful handshakes
never grant trust.

The reference verifier parses every bundle field before signature acceptance:
magic/version/flags, bundle ID, configured authority key ID, organization,
audience, subject identity and transport key, complete subject certificate,
role, sorted capability policy, issued/expiry window, and authority signature.
It rejects field mutations even when the outer bundle remains the expected
length, and it rejects a subject certificate whose identity or transport key
does not equal the separately encoded subject fields.

## Rotation, Revocation, and Recovery

Rotation is a transaction over the old identity, new identity, transport
certificate, and authorization evidence. The old node ID, old x-only key, old
transport key, and old certificate epoch are appended to revocations before
the replacement can become active. Reconnects using any revoked material fail
with `revoked`.

Recovery requires an offline-protected recovery authority or an already trusted
second manager. A relay, discovery record, backup copy, or local presence
cannot recover trust. Revocation rows are append-only and survive reconnects,
normal upgrades, and migrations. No deletion or implicit resurrection is
permitted.

Certificate IDs are not replay keys. The same signed certificate may appear in
every valid handshake during its `not-before..not-after` interval and for its
active epoch. Replay protection is category-specific: a handshake is bound to
fresh Noise ephemeral keys, ordered message state, and a new session hash;
replaying message 1 is harmless and replaying a captured message 3 into a new
handshake fails state/hash checks. Established data accepts only the next
direction sequence for the session ID, so duplicates return `replay` and gaps
close the session. Bundle IDs and manual request IDs are one-time replay keys
retained through expiry plus 24 hours. Direct Cue IDs retain their existing
durable idempotency rules. Certificate expiry, epoch revocation, and trust
state are checked on every handshake; certificate reuse does not mutate trust.

After a certificate is cryptographically valid but its node is not active in
the local registry, the session state is `authenticated_untrusted`. It is an
enrollment-only channel: it may carry an operator-confirmed request or a signed
bundle to a bounded staging area, but it cannot deliver an application
envelope, authorize a data session, execute a Cue, or mutate trust. A separate
local operator/authority operation must perform the atomic trust mutation.

## Persistence and Audit

`node.sqlite` remains the sole node-owned trust store and is separate from
`.history/runs.sqlite`. The current registry proves that both
`PRAGMA user_version` and the `metadata.schema_version` row are active schema
markers in `src/node_registry.rs`: `initialize_database` sets and validates the
pragma, while `create_schema` and `validate_schema` require the metadata row.
Version 1 therefore has both markers and a v2 migration must update both
atomically. It must not mutate version-1 rows in place or create a second
database.

Version 2 uses this normative SQLite DDL. Existing version-1 tables and their
append-only revocation/audit triggers remain unchanged. Private key bytes are
never stored in SQLite.

```sql
CREATE TABLE remote_identities (
  node_id TEXT PRIMARY KEY CHECK (length(CAST(node_id AS BLOB)) = 69),
  identity_key BLOB NOT NULL UNIQUE CHECK (length(identity_key) = 32),
  state TEXT NOT NULL CHECK (state IN ('authenticated_untrusted', 'active', 'revoked')),
  first_seen INTEGER NOT NULL CHECK (first_seen > 0),
  revoked_at INTEGER NULL CHECK (revoked_at IS NULL OR revoked_at >= first_seen)
);
CREATE TABLE trusted_peers (
  node_id TEXT PRIMARY KEY REFERENCES remote_identities(node_id),
  role INTEGER NOT NULL CHECK (role IN (1, 2)),
  capabilities BLOB NOT NULL CHECK (length(capabilities) <= 4096),
  state TEXT NOT NULL CHECK (state IN ('active', 'revoked')),
  added_at INTEGER NOT NULL CHECK (added_at > 0),
  updated_at INTEGER NOT NULL CHECK (updated_at >= added_at)
);
CREATE TABLE transport_key_epochs (
  node_id TEXT NOT NULL REFERENCES remote_identities(node_id),
  key_epoch INTEGER NOT NULL CHECK (key_epoch > 0),
  public_key BLOB NOT NULL CHECK (length(public_key) = 32),
  certificate BLOB NOT NULL CHECK (length(certificate) = 245),
  state TEXT NOT NULL CHECK (state IN ('pending', 'active', 'revoked')),
  added_at INTEGER NOT NULL CHECK (added_at > 0),
  retired_at INTEGER NULL CHECK (retired_at IS NULL OR retired_at >= added_at),
  PRIMARY KEY (node_id, key_epoch),
  UNIQUE (node_id, public_key)
);
CREATE TABLE channel_sessions (
  session_id BLOB PRIMARY KEY CHECK (length(session_id) = 32),
  node_id TEXT NOT NULL REFERENCES remote_identities(node_id),
  direction INTEGER NOT NULL CHECK (direction IN (0, 1)),
  send_sequence INTEGER NOT NULL CHECK (send_sequence >= 0),
  receive_sequence INTEGER NOT NULL CHECK (receive_sequence >= 0),
  state TEXT NOT NULL CHECK (state IN ('handshaking', 'authenticated_untrusted', 'active', 'closed')),
  started_at INTEGER NOT NULL CHECK (started_at > 0),
  last_seen INTEGER NOT NULL CHECK (last_seen >= started_at),
  expires_at INTEGER NOT NULL CHECK (expires_at >= last_seen)
);
CREATE TABLE enrollment_replays (
  replay_kind TEXT NOT NULL CHECK (replay_kind IN ('bundle', 'manual_request')),
  replay_id BLOB NOT NULL CHECK (length(replay_id) = 16),
  expires_at INTEGER NOT NULL CHECK (expires_at > 0),
  first_seen INTEGER NOT NULL CHECK (first_seen > 0),
  PRIMARY KEY (replay_kind, replay_id)
);
CREATE TABLE transport_audit (
  id INTEGER PRIMARY KEY,
  event_type TEXT NOT NULL CHECK (length(CAST(event_type AS BLOB)) BETWEEN 1 AND 64),
  node_id TEXT NOT NULL,
  session_id BLOB NULL CHECK (session_id IS NULL OR length(session_id) = 32),
  bundle_id BLOB NULL CHECK (bundle_id IS NULL OR length(bundle_id) = 16),
  direction INTEGER NULL CHECK (direction IS NULL OR direction IN (0, 1)),
  byte_count INTEGER NOT NULL CHECK (byte_count >= 0),
  outcome TEXT NOT NULL CHECK (length(CAST(outcome AS BLOB)) BETWEEN 1 AND 32),
  error_code INTEGER NULL CHECK (error_code IS NULL OR error_code BETWEEN 1000 AND 1999),
  occurred_at INTEGER NOT NULL CHECK (occurred_at > 0)
);
CREATE INDEX transport_key_epochs_state_idx ON transport_key_epochs(state, node_id);
CREATE UNIQUE INDEX transport_key_epochs_one_active
  ON transport_key_epochs(node_id) WHERE state = 'active';
CREATE INDEX channel_sessions_peer_idx ON channel_sessions(node_id, state, last_seen);
CREATE INDEX enrollment_replays_expiry_idx ON enrollment_replays(expires_at);
CREATE INDEX transport_audit_node_idx ON transport_audit(node_id, id);
CREATE INDEX transport_audit_expiry_idx ON transport_audit(occurred_at);
CREATE TRIGGER trusted_peers_require_known_identity
BEFORE INSERT ON trusted_peers
WHEN (SELECT state FROM remote_identities WHERE node_id = NEW.node_id)
  NOT IN ('authenticated_untrusted', 'active')
BEGIN SELECT RAISE(ABORT, 'trusted peer requires known identity'); END;
CREATE TRIGGER transport_key_epochs_active_require_trust
BEFORE INSERT ON transport_key_epochs
  WHEN NEW.state = 'active' AND (
  (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
  OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
)
BEGIN SELECT RAISE(ABORT, 'active transport key requires active trusted peer'); END;
CREATE TRIGGER transport_key_epochs_active_update_require_trust
BEFORE UPDATE OF state ON transport_key_epochs
  WHEN NEW.state = 'active' AND (
  (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
  OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
)
BEGIN SELECT RAISE(ABORT, 'active transport key requires active trusted peer'); END;
CREATE TRIGGER remote_identities_no_untrusted_trust_update
BEFORE UPDATE OF state ON remote_identities
WHEN NEW.state = 'active' AND NOT EXISTS (
  SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id
)
BEGIN SELECT RAISE(ABORT, 'active identity requires trusted peer'); END;
CREATE TRIGGER trusted_peers_no_identity_demotion
BEFORE UPDATE OF state ON remote_identities
WHEN NEW.state <> 'active' AND EXISTS (
  SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active'
)
BEGIN SELECT RAISE(ABORT, 'trusted peer must be revoked before identity demotion'); END;
CREATE TRIGGER channel_sessions_active_requires_trust
BEFORE INSERT ON channel_sessions
WHEN NEW.state = 'active' AND (
  (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
  OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
)
BEGIN SELECT RAISE(ABORT, 'active session requires active trusted peer'); END;
CREATE TRIGGER channel_sessions_active_update_requires_trust
BEFORE UPDATE OF state, node_id ON channel_sessions
WHEN NEW.state = 'active' AND (
  (SELECT state FROM remote_identities WHERE node_id = NEW.node_id) <> 'active'
  OR NOT EXISTS (SELECT 1 FROM trusted_peers WHERE node_id = NEW.node_id AND state = 'active')
)
BEGIN SELECT RAISE(ABORT, 'active session requires active trusted peer'); END;
CREATE TRIGGER transport_key_epochs_monotonic_insert
BEFORE INSERT ON transport_key_epochs
WHEN NEW.key_epoch <= COALESCE((SELECT MAX(key_epoch) FROM transport_key_epochs WHERE node_id = NEW.node_id), 0)
BEGIN SELECT RAISE(ABORT, 'transport key epoch must increase'); END;
CREATE TRIGGER transport_key_epochs_monotonic_update
BEFORE UPDATE OF key_epoch ON transport_key_epochs
WHEN NEW.key_epoch <= COALESCE((SELECT MAX(key_epoch) FROM transport_key_epochs WHERE node_id = NEW.node_id AND key_epoch <> OLD.key_epoch), 0)
BEGIN SELECT RAISE(ABORT, 'transport key epoch must increase'); END;
CREATE TRIGGER remote_identities_no_delete
BEFORE DELETE ON remote_identities
BEGIN SELECT RAISE(ABORT, 'remote identities are retained'); END;
CREATE TRIGGER trusted_peers_no_delete
BEFORE DELETE ON trusted_peers
BEGIN SELECT RAISE(ABORT, 'trusted peer history is retained'); END;
CREATE TRIGGER transport_key_epochs_no_delete
BEFORE DELETE ON transport_key_epochs
BEGIN SELECT RAISE(ABORT, 'transport key epochs are retained'); END;
CREATE TRIGGER revoked_identity_no_resurrection
BEFORE UPDATE OF state ON remote_identities
WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
BEGIN SELECT RAISE(ABORT, 'revoked identity cannot be resurrected'); END;
CREATE TRIGGER revoked_trusted_peer_no_resurrection
BEFORE UPDATE OF state ON trusted_peers
WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
BEGIN SELECT RAISE(ABORT, 'revoked trust cannot be resurrected'); END;
CREATE TRIGGER revoked_transport_epoch_no_resurrection
BEFORE UPDATE OF state ON transport_key_epochs
WHEN OLD.state = 'revoked' AND NEW.state <> 'revoked'
BEGIN SELECT RAISE(ABORT, 'revoked transport epoch cannot be resurrected'); END;
```

The exact migration is: open the existing database with WAL, foreign keys, and
the 2-second busy timeout; begin `IMMEDIATE`; require `PRAGMA user_version = 1`,
`metadata.schema_version = '1'`, `PRAGMA integrity_check = 'ok'`, and the exact
version-1 table/column set. Create the v2 tables, copy retained revocations and
audit rows, create the triggers and indexes above, update the metadata row to
`schema_version = '2'`, execute `PRAGMA user_version = 2`, and commit. The
transaction must verify that every active identity has exactly one active
trusted peer and at most one active transport epoch before commit. Reopen and
validate every table, column, constraint, trigger, index, both schema markers,
metadata values, and foreign keys. Any failed step rolls back and prevents
direct mode. Downgrading a version-2 database is unsupported and fails closed.
No private material is copied or derived inside the migration.

The v1 mapping is exact and rejects lossy or non-canonical input:

| v1 `peers` field | v2 mapping |
|---|---|
| `node_id` | `remote_identities.node_id`, unchanged after lowercase `omk1_` validation |
| lowercase `public_key` | `remote_identities.identity_key`, decode exactly 64 lowercase hex characters to 32 bytes; recompute and compare `node_id` |
| `role` `conductor` / `performer` | `trusted_peers.role` `1` / `2` |
| `state` `pending` | remote identity `authenticated_untrusted`; no trusted peer or active epoch |
| `state` `active` | remote identity `active` plus one `trusted_peers` row with `state = 'active'` |
| `state` `suspended` | remote identity `authenticated_untrusted` plus a retained revoked trust row; never active |
| `state` `revoked` | remote identity `revoked` plus a retained revoked trust row; never resurrectable |
| canonical `capabilities_json` | `trusted_peers.capabilities`, preserving the validated sorted JSON bytes |
| canonical UTC RFC3339-millisecond timestamps | v2 integer Unix seconds only after parsing UTC and requiring zero sub-second remainder; otherwise migration rejects rather than rounds |
| v1 peer row with no transport key | no `transport_key_epochs` row; v2 transport enrollment must provide a new certificate |

The existing v1 tables remain unchanged during this additive migration, so
`source`, `last_seen`, and the v1 peer audit/revocation records remain available
in their original canonical columns; they are not silently discarded. A v1 row
that cannot satisfy the v2 identity, role, capability, timestamp, or state
mapping aborts the transaction.

Every 30-second cleanup transaction deletes `enrollment_replays` only where
`expires_at < now - 86400`, and deletes closed session rows older than 86400
seconds. It retains at most 1,000,000 live replay rows and 1,000,000 audit rows;
when either cap would be exceeded, new enrollment or trust-changing audit
writes fail closed rather than deleting live evidence. Audit rows older than
365 days may be archived only by an explicit offline operator operation; the
database operation never silently drops them. Cleanup is bounded to 10,000
rows per transaction and resumes on the next 30-second interval.

Trust insertion, replay insertion, certificate-epoch update, revocation, and
the corresponding audit row occur in one `BEGIN IMMEDIATE` transaction. The
transaction verifies the peer is not self, checks retained revocations, uses
`INSERT ... ON CONFLICT` only to return a duplicate without mutation, and
commits all rows or none. A channel cannot call this transaction directly;
only the local manual/operator or configured authority operation can invoke it.

Audit rows may contain event type, outcome, local node ID, redacted remote node
ID, session ID, bundle ID, direction, byte count, coarse error code, and UTC
time. They must never contain private keys, shared secrets, plaintext, raw
frames, full signatures, manual codes, bearer credentials, or authority
secrets. Trust-changing audit writes are in the same SQLite transaction as the
trust mutation and fail closed. Non-authorizing telemetry is bounded and may
drop only with a local counter.

## Canonical E2E Topology

Every later direct-channel E2E slice uses one independently stateful topology:

| Container | Purpose | Required isolation |
|---|---|---|
| `conductor` | Manager-side node service | Own identity, transport key, registry, and state volume |
| `performer-a` | First peer | Own identity, transport key, registry, and state volume |
| `performer-b` | Second peer and rotation/replay peer | Own identity, transport key, registry, and state volume |
| `attacker` | Active network adversary/proxy | No node state; may drop, duplicate, reorder, delay, truncate, mutate, or oversize traffic |
| `relay` | Optional untrusted delivery path | No trust authority and no node-volume access; may omit, retain, censor, or replay data |

All containers use the built binary, explicit configuration, fresh state
volumes, and a private test network. No container mounts another container's
`identity.key`, transport key, `node.sqlite`, or socket. The harness controls
deterministic clocks, bounded skew, restarts, and redacted result collection.

## Required Adversarial Coverage

The executable contract and future E2E tests must cover:

| Case | Mutation or fault | Required result |
|---|---|---|
| Spoofing | Replace BIP-340 identity, authority, certificate, or peer binding | Reject before trust or delivery |
| Replay | Re-submit handshake, bundle, manual request, frame, or Cue ID | Reject or return the specified idempotent result without re-execution |
| Expiry | Move certificate/bundle beyond its validity window | `expired`, no state mutation |
| Downgrade | Alter version, pattern, cipher, hash, prologue, or field list | Reject; no fallback |
| Identity mismatch | Pair an x-only key, node ID, certificate, or transport key incorrectly | `identity_mismatch` |
| Malformed input | Truncate, duplicate, reorder, unknown, invalid, or noncanonical fields | Bounded rejection, no panic, no mutation |
| Oversized input | Exceed every frame, handshake, bundle, capability, queue, or peer limit | `message_too_large` before allocation or crypto |
| Clock skew | Test accepted boundary and one tick beyond | Deterministic inclusive/exclusive behavior |
| Interrupted handshake | Drop each message, restart either node, reuse partial state | No half-enrolled peer, secret leak, or unsafe resumption |
| Revocation/rotation | Reconnect with old and replacement material, stale bundle | Old material rejected; replacement transactional |
| Nonce/rekey | Hit rekey threshold and approach counter exhaustion | Standard rekey; close before wrap |

## Public Vectors and Feasibility

`tests/fixtures/direct_transport_feasibility.toml` is format version 2 and
contains independent initiator and responder public test vectors. It records
the exact certificates, fixed-ephemeral Noise XX messages, handshake hash, and
one ciphertext in each direction. Private values in the vector are published
RFC test inputs and are explicitly not production secrets. The test
`tests/direct_transport_contract.rs` verifies certificate signatures, the real
`snow` handshake and transport ciphertext, exact framing, and mutations of
certificate/frame bytes.

The pinned pure-Rust dependency is a dev dependency for the contract fixture.
Contract-freeze task #2718 adds no production transport; task #2719 owns the
first production implementation and must promote the dependency and the
fixture's `production_transport_claim` only when its runtime and E2E gates pass.
The selected crate has no required OS crypto ABI with the selected resolver.
The repository's locked CI matrix supplies Linux, macOS, and Windows build
feasibility for the dependency graph; this Linux development environment does
not claim native execution on the other two platforms. The later production
implementation remains responsible for running the locked test/build matrix
on all three targets before release. Local evidence is Linux: the full locked
test, clippy, format, and diff checks pass. A local Windows GNU check was
attempted but cannot run because `x86_64-w64-mingw32-gcc` is not installed;
macOS targets are not installed here. No hosted macOS or Windows result is
claimed by this task.

Rejected alternatives remain rejected for this contract: `libp2p-noise` has an
experimental wire protocol and is X25519-only; HPKE is not an ordered mutually
authenticated channel; and `k256` ECDH is only a primitive, so a custom channel
composition would be new security-sensitive protocol code. No TLS, custom
Noise resolver, key conversion, or one-key secp256k1 transport variant may be
introduced without a new owner-approved contract.

## References

- [Noise Protocol Framework](https://noiseprotocol.org/noise.html)
- [`snow` 0.10.0 documentation](https://docs.rs/snow/0.10.0/snow/)
- [RFC 7748 X25519](https://www.rfc-editor.org/rfc/rfc7748)
- [`k256` repository security notes](https://github.com/RustCrypto/elliptic-curves/tree/master/k256)
- [NCC Group RustCrypto audit report](https://www.nccgroup.com/us/research-blog/public-report-entropyrust-cryptography-review/)
