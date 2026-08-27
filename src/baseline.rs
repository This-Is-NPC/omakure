//! The signed baseline manifest: the versioned set of scripts a fleet ships.
//!
//! Everything before item 8 was built so that a Cue "names a script and never
//! carries one". A baseline is the first artefact that carries code, so the
//! question it has to answer is not "did this arrive" but "is this exactly the
//! set someone signed".
//!
//! Two decisions carry that weight.
//!
//! **One signature over the set, not one per script.** The manifest names every
//! script with its content hash and the publisher signs the manifest. A
//! per-script signature would prove each file's origin and still say nothing
//! about whether the files are the *set* that was published — a receiver
//! holding two of three validly signed scripts could not tell it was short one.
//! Signing the set makes "which files, and which bytes" a single verifiable
//! statement, and multiplies no verification surface to do it.
//!
//! **The identity of a baseline is derived, never declared.** [`baseline_id`]
//! is a hash of the entry list alone. A publisher cannot name it, so two
//! different sets can never claim the same version, and a Performer can
//! recompute it from the files it actually holds without knowing who signed
//! them or when. That is what makes a later drift answer checkable rather than
//! merely reported.
//!
//! [`baseline_id`]: SignedBaselineManifest::baseline_id

use k256::schnorr::{
    signature::hazmat::{PrehashSigner, PrehashVerifier},
    Signature, SigningKey, VerifyingKey,
};
use sha2::{Digest, Sha256};
use std::fmt;
use thiserror::Error;

pub const VERSION: u8 = 1;

/// Domain separator for the manifest signature.
pub const DOMAIN: &[u8] = b"omakure/baseline-manifest/v1\0";

/// Domain separator for the derived baseline identity.
///
/// Separate from [`DOMAIN`]: the same bytes must not hash alike as "the thing
/// that was signed" and as "the name of the set".
pub const BASELINE_ID_DOMAIN: &[u8] = b"omakure/baseline-id/v1\0";

pub const PUBLISHER_ID_BYTES: usize = 16;
pub const BASELINE_ID_BYTES: usize = 32;
pub const SCRIPT_HASH_BYTES: usize = 32;
pub const SIGNATURE_BYTES: usize = 64;
pub const PUBLISHER_KEY_BYTES: usize = 32;

pub const MAX_ENTRIES: usize = 256;
pub const MAX_ENTRY_PATH_BYTES: usize = 128;
pub const MAX_SCRIPT_BYTES: usize = 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_ORGANIZATION_BYTES: usize = 128;
pub const FUTURE_SKEW_SECONDS: u64 = 300;
pub const MAX_LIFETIME_SECONDS: u64 = 30 * 24 * 60 * 60;

const MAGIC: &[u8; 4] = b"OMBM";

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BaselineError {
    #[error("invalid baseline manifest")]
    Invalid,
    #[error("baseline manifest is too large")]
    TooLarge,
    #[error("baseline manifest is expired")]
    Expired,
    #[error("baseline manifest publisher is not trusted")]
    PublisherUnknown,
    #[error("baseline manifest publisher is revoked")]
    PublisherRevoked,
    #[error("baseline manifest organization does not match local policy")]
    OrganizationMismatch,
    #[error("baseline manifest signature is invalid")]
    SignatureMismatch,
    #[error("baseline content does not match the signed manifest")]
    ContentMismatch,
}

/// A publisher key as a receiver records it, mirroring
/// [`crate::enrollment::BundleAuthority`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselinePublisherKey {
    pub key_id: [u8; PUBLISHER_ID_BYTES],
    pub public_key: [u8; PUBLISHER_KEY_BYTES],
    pub revoked: bool,
}

/// One script in the set, named by path and pinned by content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineEntry {
    pub path: String,
    pub content_hash: [u8; SCRIPT_HASH_BYTES],
}

/// The set, signed once.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedBaselineManifest {
    pub publisher_key_id: [u8; PUBLISHER_ID_BYTES],
    pub organization: String,
    pub entries: Vec<BaselineEntry>,
    pub issued_at: u64,
    pub expires_at: u64,
    pub publisher_signature: [u8; SIGNATURE_BYTES],
}

impl fmt::Debug for SignedBaselineManifest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedBaselineManifest")
            .field("publisher_key_id", &hex(&self.publisher_key_id))
            .field("organization", &self.organization)
            .field("entries", &self.entries.len())
            .field("baseline_id", &self.baseline_id().map(|id| hex(&id)).ok())
            .field("issued_at", &self.issued_at)
            .field("expires_at", &self.expires_at)
            .field("publisher_signature", &"<redacted>")
            .finish()
    }
}

impl SignedBaselineManifest {
    /// Build and sign a manifest from the script bodies themselves.
    ///
    /// The hashes are computed here rather than accepted from the caller: a
    /// manifest whose recorded hash disagrees with the bytes it was published
    /// with must not be constructible through the supported path, or the whole
    /// content binding is advisory.
    pub fn sign_with_material(
        publisher_private_key: &[u8],
        publisher_key_id: [u8; PUBLISHER_ID_BYTES],
        organization: String,
        scripts: &[(String, Vec<u8>)],
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, BaselineError> {
        let signing_key =
            SigningKey::from_slice(publisher_private_key).map_err(|_| BaselineError::Invalid)?;
        let mut entries = Vec::with_capacity(scripts.len());
        for (path, body) in scripts {
            if body.len() > MAX_SCRIPT_BYTES {
                return Err(BaselineError::TooLarge);
            }
            entries.push(BaselineEntry {
                path: path.clone(),
                content_hash: hash_script(body),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let mut manifest = Self {
            publisher_key_id,
            organization,
            entries,
            issued_at,
            expires_at,
            publisher_signature: [0; SIGNATURE_BYTES],
        };
        let digest = hash_domain(&manifest.unsigned_bytes()?, DOMAIN);
        manifest.publisher_signature = signing_key
            .sign_prehash(&digest)
            .map_err(|_| BaselineError::Invalid)?
            .to_bytes();
        manifest.verify_shape()?;
        Ok(manifest)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, BaselineError> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(BaselineError::TooLarge);
        }
        let mut cursor = Cursor::new(bytes);
        if cursor.take(4)? != MAGIC || cursor.byte()? != VERSION || cursor.take(2)? != [0, 0] {
            return Err(BaselineError::Invalid);
        }
        let publisher_key_id = cursor.array::<PUBLISHER_ID_BYTES>()?;
        let organization_length = usize::from(cursor.u16()?);
        let organization = cursor.text(organization_length)?;
        let entry_count = usize::from(cursor.u16()?);
        if entry_count > MAX_ENTRIES {
            return Err(BaselineError::Invalid);
        }
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            let length = usize::from(cursor.u16()?);
            let path = cursor.text(length)?;
            let content_hash = cursor.array::<SCRIPT_HASH_BYTES>()?;
            entries.push(BaselineEntry { path, content_hash });
        }
        let issued_at = cursor.u64()?;
        let expires_at = cursor.u64()?;
        let publisher_signature = cursor.array::<SIGNATURE_BYTES>()?;
        if cursor.remaining() != 0 {
            return Err(BaselineError::Invalid);
        }
        let manifest = Self {
            publisher_key_id,
            organization,
            entries,
            issued_at,
            expires_at,
            publisher_signature,
        };
        manifest.verify_shape()?;
        Ok(manifest)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.unsigned_bytes()
            .expect("signed baseline manifest must be valid")
            .into_iter()
            .chain(self.publisher_signature)
            .collect()
    }

    /// The derived name of this set.
    ///
    /// Covers the entry list and nothing else, so a Performer can recompute it
    /// from the scripts on its own disk. Adding the publisher, the
    /// organization, or a timestamp would make the same installed files answer
    /// differently depending on who pushed them, which is precisely what a
    /// drift comparison must not depend on.
    pub fn baseline_id(&self) -> Result<[u8; BASELINE_ID_BYTES], BaselineError> {
        Ok(hash_domain(
            &entry_bytes(&self.entries)?,
            BASELINE_ID_DOMAIN,
        ))
    }

    pub fn verify(
        &self,
        publisher: &BaselinePublisherKey,
        organization: &str,
        now: u64,
    ) -> Result<(), BaselineError> {
        self.verify_shape()?;
        if self.publisher_key_id != publisher.key_id {
            return Err(BaselineError::PublisherUnknown);
        }
        if publisher.revoked {
            return Err(BaselineError::PublisherRevoked);
        }
        if self.organization != organization {
            return Err(BaselineError::OrganizationMismatch);
        }
        if now.saturating_add(FUTURE_SKEW_SECONDS) < self.issued_at || now >= self.expires_at {
            return Err(BaselineError::Expired);
        }
        let key = VerifyingKey::from_slice(&publisher.public_key)
            .map_err(|_| BaselineError::PublisherUnknown)?;
        let signature =
            Signature::from_slice(&self.publisher_signature).map_err(|_| BaselineError::Invalid)?;
        let digest = hash_domain(&self.unsigned_bytes()?, DOMAIN);
        key.verify_prehash(&digest, &signature)
            .map_err(|_| BaselineError::SignatureMismatch)
    }

    /// Bind script bodies to this manifest, all of them or none.
    ///
    /// This is the only way to reach the bytes of a baseline, and it is
    /// deliberately not incremental: there is no per-entry accessor that hands
    /// back a verified script, so an install path cannot walk the set writing
    /// files until one fails. One script whose bytes do not match its recorded
    /// hash — or one missing, extra, or duplicated path — rejects the whole
    /// baseline, which is what keeps a partial install off the map of reachable
    /// states.
    pub fn bind(self, scripts: Vec<(String, Vec<u8>)>) -> Result<VerifiedBaseline, BaselineError> {
        self.verify_shape()?;
        if scripts.len() != self.entries.len() {
            return Err(BaselineError::ContentMismatch);
        }
        let mut ordered = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let mut matches = scripts.iter().filter(|(path, _)| path == &entry.path);
            let (_, body) = matches.next().ok_or(BaselineError::ContentMismatch)?;
            if matches.next().is_some() {
                return Err(BaselineError::ContentMismatch);
            }
            if hash_script(body) != entry.content_hash {
                return Err(BaselineError::ContentMismatch);
            }
            ordered.push((entry.path.clone(), body.clone()));
        }
        Ok(VerifiedBaseline {
            manifest: self,
            scripts: ordered,
        })
    }

    fn unsigned_bytes(&self) -> Result<Vec<u8>, BaselineError> {
        self.verify_shape()?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&0_u16.to_be_bytes());
        bytes.extend_from_slice(&self.publisher_key_id);
        bytes.extend_from_slice(
            &u16::try_from(self.organization.len())
                .map_err(|_| BaselineError::TooLarge)?
                .to_be_bytes(),
        );
        bytes.extend_from_slice(self.organization.as_bytes());
        bytes.extend_from_slice(&entry_bytes(&self.entries)?);
        bytes.extend_from_slice(&self.issued_at.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at.to_be_bytes());
        if bytes.len() + SIGNATURE_BYTES > MAX_MANIFEST_BYTES {
            return Err(BaselineError::TooLarge);
        }
        Ok(bytes)
    }

    fn verify_shape(&self) -> Result<(), BaselineError> {
        if self.organization.len() > MAX_ORGANIZATION_BYTES
            || self
                .organization
                .bytes()
                .any(|byte| byte.is_ascii_control())
        {
            return Err(BaselineError::Invalid);
        }
        // An empty set is refused rather than signed: "install nothing" and
        // "nothing was published" would become the same artefact, and a
        // delivery path could report the second as a successful first.
        if self.entries.is_empty() || self.entries.len() > MAX_ENTRIES {
            return Err(BaselineError::Invalid);
        }
        let mut previous: Option<&str> = None;
        for entry in &self.entries {
            validate_entry_path(&entry.path)?;
            // Strictly ascending also rules out duplicates, so one path can
            // never carry two hashes and leave the winner to the reader.
            if previous.is_some_and(|previous| previous >= entry.path.as_str()) {
                return Err(BaselineError::Invalid);
            }
            previous = Some(entry.path.as_str());
        }
        if self.expires_at <= self.issued_at
            || self.expires_at - self.issued_at > MAX_LIFETIME_SECONDS
        {
            return Err(BaselineError::Invalid);
        }
        Ok(())
    }
}

/// A manifest whose every script has been checked against its recorded hash.
///
/// Only [`SignedBaselineManifest::bind`] constructs one, so holding this type
/// is the proof that the whole set matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBaseline {
    manifest: SignedBaselineManifest,
    scripts: Vec<(String, Vec<u8>)>,
}

impl VerifiedBaseline {
    pub fn manifest(&self) -> &SignedBaselineManifest {
        &self.manifest
    }

    pub fn baseline_id(&self) -> Result<[u8; BASELINE_ID_BYTES], BaselineError> {
        self.manifest.baseline_id()
    }

    /// The scripts, in manifest order.
    pub fn scripts(&self) -> &[(String, Vec<u8>)] {
        &self.scripts
    }
}

/// The recorded hash of one script body.
///
/// Domain-separated rather than a bare digest, so a hash computed for some
/// other purpose can never be presented as a baseline entry. The consequence
/// is deliberate: this is not `sha256sum` output, and anything that later
/// re-checks a script on disk has to call this, not reimplement it.
pub fn hash_script(body: &[u8]) -> [u8; SCRIPT_HASH_BYTES] {
    hash_domain(body, b"omakure/baseline-script/v1\0")
}

/// The canonical bytes of the entry list, shared by the signature preimage and
/// the derived baseline id so the two can never describe different sets.
fn entry_bytes(entries: &[BaselineEntry]) -> Result<Vec<u8>, BaselineError> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(
        &u16::try_from(entries.len())
            .map_err(|_| BaselineError::TooLarge)?
            .to_be_bytes(),
    );
    for entry in entries {
        bytes.extend_from_slice(
            &u16::try_from(entry.path.len())
                .map_err(|_| BaselineError::TooLarge)?
                .to_be_bytes(),
        );
        bytes.extend_from_slice(entry.path.as_bytes());
        bytes.extend_from_slice(&entry.content_hash);
    }
    Ok(bytes)
}

/// A baseline path names a script inside a workspace and nothing else.
fn validate_entry_path(path: &str) -> Result<(), BaselineError> {
    if path.is_empty() || path.len() > MAX_ENTRY_PATH_BYTES {
        return Err(BaselineError::Invalid);
    }
    if path.bytes().any(|byte| {
        !(byte.is_ascii_lowercase()
            || byte.is_ascii_uppercase()
            || byte.is_ascii_digit()
            || b"._-/".contains(&byte))
    }) {
        return Err(BaselineError::Invalid);
    }
    let mut components = path.split('/').peekable();
    let mut last = None;
    while let Some(component) = components.next() {
        // An empty component covers the leading slash, the trailing slash, and
        // the doubled separator in one test.
        if component.is_empty() || component.starts_with('.') {
            return Err(BaselineError::Invalid);
        }
        if components.peek().is_none() {
            last = Some(component);
        }
    }
    let last = last.ok_or(BaselineError::Invalid)?;
    let extension = last.rsplit_once('.').ok_or(BaselineError::Invalid)?.1;
    if !crate::runtime::script_extensions().contains(&extension) {
        return Err(BaselineError::Invalid);
    }
    Ok(())
}

fn hash_domain(bytes: &[u8], domain: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    digest.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], BaselineError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(BaselineError::Invalid)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(BaselineError::Invalid)?;
        self.offset = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8, BaselineError> {
        Ok(*self.take(1)?.first().ok_or(BaselineError::Invalid)?)
    }

    fn u16(&mut self) -> Result<u16, BaselineError> {
        Ok(u16::from_be_bytes(
            self.take(2)?
                .try_into()
                .map_err(|_| BaselineError::Invalid)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, BaselineError> {
        Ok(u64::from_be_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| BaselineError::Invalid)?,
        ))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], BaselineError> {
        self.take(N)?.try_into().map_err(|_| BaselineError::Invalid)
    }

    fn text(&mut self, length: usize) -> Result<String, BaselineError> {
        std::str::from_utf8(self.take(length)?)
            .map(str::to_string)
            .map_err(|_| BaselineError::Invalid)
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISSUED_AT: u64 = 1_800_000_000;
    const EXPIRES_AT: u64 = 1_800_003_600;

    fn publisher(scalar: u8) -> (SigningKey, BaselinePublisherKey) {
        let signing_key = SigningKey::from_slice(&[scalar; 32]).expect("valid scalar");
        let mut public_key = [0u8; PUBLISHER_KEY_BYTES];
        public_key.copy_from_slice(signing_key.verifying_key().to_bytes().as_slice());
        let mut key_id = [0u8; PUBLISHER_ID_BYTES];
        key_id.copy_from_slice(&Sha256::digest(public_key)[..PUBLISHER_ID_BYTES]);
        (
            signing_key,
            BaselinePublisherKey {
                key_id,
                public_key,
                revoked: false,
            },
        )
    }

    fn scripts() -> Vec<(String, Vec<u8>)> {
        vec![
            ("ops/deploy.sh".to_string(), b"echo deploy\n".to_vec()),
            ("ops/rollback.sh".to_string(), b"echo rollback\n".to_vec()),
            ("audit.py".to_string(), b"print('audit')\n".to_vec()),
        ]
    }

    fn sign(scalar: u8, bodies: &[(String, Vec<u8>)]) -> SignedBaselineManifest {
        let (signing_key, key) = publisher(scalar);
        SignedBaselineManifest::sign_with_material(
            signing_key.to_bytes().as_ref(),
            key.key_id,
            "acme".to_string(),
            bodies,
            ISSUED_AT,
            EXPIRES_AT,
        )
        .expect("sign the manifest")
    }

    /// The signature covers the set, so touching any part of it breaks.
    #[test]
    fn a_signed_manifest_verifies_and_an_altered_entry_does_not() {
        let (_, key) = publisher(3);
        let manifest = sign(3, &scripts());

        manifest
            .verify(&key, "acme", ISSUED_AT + 1)
            .expect("the manifest the publisher signed must verify");

        let mut tampered = manifest.clone();
        tampered.entries[0].content_hash[0] ^= 0xff;
        assert_eq!(
            tampered.verify(&key, "acme", ISSUED_AT + 1),
            Err(BaselineError::SignatureMismatch),
            "swapping the hash of one script must invalidate the whole manifest"
        );

        let (_, other) = publisher(5);
        assert_eq!(
            manifest.verify(&other, "acme", ISSUED_AT + 1),
            Err(BaselineError::PublisherUnknown),
            "a manifest must not verify under a publisher that did not sign it"
        );
        assert_eq!(
            manifest.verify(&key, "other-org", ISSUED_AT + 1),
            Err(BaselineError::OrganizationMismatch)
        );
    }

    /// The version identifier names the content, so neither side can lie about
    /// which baseline it holds.
    #[test]
    fn the_baseline_id_is_derived_from_the_set_alone() {
        let bodies = scripts();
        let mine = sign(3, &bodies);

        let mut later = bodies.clone();
        later.reverse();
        let theirs = SignedBaselineManifest::sign_with_material(
            publisher(9).0.to_bytes().as_ref(),
            publisher(9).1.key_id,
            "different-org".to_string(),
            &later,
            ISSUED_AT + 500,
            EXPIRES_AT + 500,
        )
        .expect("sign");

        assert_eq!(
            mine.baseline_id().expect("id"),
            theirs.baseline_id().expect("id"),
            "a different publisher, org, order, and time over the same scripts \
             is the same baseline; anything else makes drift depend on the pusher"
        );

        let mut changed = bodies;
        changed[0].1.push(b'!');
        assert_ne!(
            mine.baseline_id().expect("id"),
            sign(3, &changed).baseline_id().expect("id"),
            "one changed byte must name a different baseline"
        );
    }

    /// One bad script rejects the set; there is no way to reach the good ones.
    #[test]
    fn a_single_mismatched_script_rejects_the_whole_baseline() {
        let bodies = scripts();
        let manifest = sign(3, &bodies);

        let bound = manifest
            .clone()
            .bind(bodies.clone())
            .expect("the published bodies must bind");
        assert_eq!(bound.scripts().len(), bodies.len());
        assert_eq!(
            bound.baseline_id().expect("id"),
            manifest.baseline_id().expect("id")
        );

        let mut corrupted = bodies.clone();
        corrupted[1].1.push(b'\n');
        assert_eq!(
            manifest.clone().bind(corrupted),
            Err(BaselineError::ContentMismatch),
            "a script whose bytes do not match its recorded hash must void the set"
        );

        let mut short = bodies.clone();
        short.pop();
        assert_eq!(
            manifest.clone().bind(short),
            Err(BaselineError::ContentMismatch),
            "a missing script must void the set rather than install what arrived"
        );

        let mut extra = bodies.clone();
        extra.push(("extra.sh".to_string(), b"echo extra\n".to_vec()));
        assert_eq!(
            manifest.clone().bind(extra),
            Err(BaselineError::ContentMismatch),
            "an unlisted script must void the set"
        );

        let mut duplicated = bodies;
        duplicated[2] = duplicated[0].clone();
        assert_eq!(
            manifest.bind(duplicated),
            Err(BaselineError::ContentMismatch),
            "one path supplied twice must void the set"
        );
    }

    #[test]
    fn a_manifest_survives_its_own_encoding_and_rejects_trailing_bytes() {
        let manifest = sign(3, &scripts());
        let encoded = manifest.encode();

        assert_eq!(
            SignedBaselineManifest::decode(&encoded).expect("decode"),
            manifest
        );

        let mut trailing = encoded.clone();
        trailing.push(0);
        assert_eq!(
            SignedBaselineManifest::decode(&trailing),
            Err(BaselineError::Invalid),
            "trailing bytes must not be silently ignored"
        );

        let mut truncated = encoded;
        truncated.pop();
        assert_eq!(
            SignedBaselineManifest::decode(&truncated),
            Err(BaselineError::Invalid)
        );
    }

    #[test]
    fn a_manifest_outside_its_window_or_from_a_revoked_publisher_is_refused() {
        let (_, key) = publisher(3);
        let manifest = sign(3, &scripts());

        assert_eq!(
            manifest.verify(&key, "acme", EXPIRES_AT),
            Err(BaselineError::Expired),
            "expiry is inclusive of the instant it names"
        );
        assert_eq!(
            manifest.verify(&key, "acme", ISSUED_AT - FUTURE_SKEW_SECONDS - 1),
            Err(BaselineError::Expired),
            "a manifest from beyond the tolerated skew must not verify"
        );

        let revoked = BaselinePublisherKey {
            revoked: true,
            ..key
        };
        assert_eq!(
            manifest.verify(&revoked, "acme", ISSUED_AT + 1),
            Err(BaselineError::PublisherRevoked)
        );
    }

    /// A baseline path names a script inside a workspace and nothing else.
    #[test]
    fn entry_paths_cannot_escape_the_workspace_or_name_a_non_script() {
        for path in [
            "../escape.sh",
            "/etc/cron.sh",
            "ops//deploy.sh",
            "ops/",
            ".hidden.sh",
            "ops/.hidden/deploy.sh",
            "notes.txt",
            "noextension",
            "ops/deploy.sh\0",
            "ops/deploy sh",
        ] {
            assert_eq!(
                validate_entry_path(path),
                Err(BaselineError::Invalid),
                "{path:?} must not be nameable by a baseline"
            );
        }
        for path in ["deploy.sh", "ops/deploy.bash", "a/b/c/task.lua", "audit.py"] {
            assert_eq!(
                validate_entry_path(path),
                Ok(()),
                "{path:?} is an ordinary workspace script"
            );
        }
    }

    #[test]
    fn an_empty_baseline_is_not_signable() {
        let (signing_key, key) = publisher(3);
        assert_eq!(
            SignedBaselineManifest::sign_with_material(
                signing_key.to_bytes().as_ref(),
                key.key_id,
                "acme".to_string(),
                &[],
                ISSUED_AT,
                EXPIRES_AT,
            ),
            Err(BaselineError::Invalid),
            "\"install nothing\" must not be signable as a baseline"
        );
    }

    /// Canonical order is imposed by the signer, not asked of the caller.
    #[test]
    fn entries_are_ordered_by_path_whatever_order_they_arrive_in() {
        let bodies = scripts();
        let mut shuffled = bodies.clone();
        shuffled.rotate_left(1);

        let manifest = sign(3, &bodies);
        assert_eq!(
            manifest
                .entries
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["audit.py", "ops/deploy.sh", "ops/rollback.sh"]
        );
        assert_eq!(
            manifest,
            sign(3, &shuffled),
            "the same set must produce the same signed bytes regardless of input order"
        );
    }

    #[test]
    fn a_duplicate_path_cannot_be_signed() {
        let (signing_key, key) = publisher(3);
        let duplicated = vec![
            ("deploy.sh".to_string(), b"one\n".to_vec()),
            ("deploy.sh".to_string(), b"two\n".to_vec()),
        ];
        assert_eq!(
            SignedBaselineManifest::sign_with_material(
                signing_key.to_bytes().as_ref(),
                key.key_id,
                "acme".to_string(),
                &duplicated,
                ISSUED_AT,
                EXPIRES_AT,
            ),
            Err(BaselineError::Invalid),
            "one path carrying two bodies must not be signable"
        );
    }

    #[test]
    fn an_unbounded_lifetime_is_not_signable() {
        let (signing_key, key) = publisher(3);
        assert_eq!(
            SignedBaselineManifest::sign_with_material(
                signing_key.to_bytes().as_ref(),
                key.key_id,
                "acme".to_string(),
                &scripts(),
                ISSUED_AT,
                ISSUED_AT + MAX_LIFETIME_SECONDS + 1,
            ),
            Err(BaselineError::Invalid)
        );
    }
}
