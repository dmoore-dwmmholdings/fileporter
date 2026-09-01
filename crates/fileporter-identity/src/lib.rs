//! Fileporter v1 device identity and pairing cryptography.

use std::time::SystemTime;

use data_encoding::BASE32_NOPAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use thiserror::Error;
use zeroize::Zeroizing;

pub const PAIRING_PROTOCOL_VERSION: u16 = 1;
const ID_DOMAIN: &[u8] = b"fileporter/device-id/v1";
const SIGNING_DOMAIN: &[u8] = b"fileporter/pairing-signature/v1";
const SAS_DOMAIN: &[u8] = b"fileporter/pairing-sas/v1";
const TRANSCRIPT_DOMAIN: &[u8] = b"fileporter/pairing-transcript/v1";

#[derive(Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    secret: Zeroizing<[u8; 32]>,
}

impl DeviceIdentity {
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self {
            secret: Zeroizing::new(signing_key.to_bytes()),
        }
    }

    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        Self {
            secret: Zeroizing::new(secret),
        }
    }

    pub fn export_secret_bytes(&self) -> Zeroizing<[u8; 32]> {
        self.secret.clone()
    }

    pub fn public_identity(&self) -> DevicePublicIdentity {
        DevicePublicIdentity::from_verifying_key(
            SigningKey::from_bytes(&self.secret).verifying_key(),
        )
    }

    pub fn sign_domain_separated(&self, domain: &[u8], message: &[u8]) -> [u8; 64] {
        SigningKey::from_bytes(&self.secret)
            .sign(&domain_message(domain, message))
            .to_bytes()
    }

    pub fn sign_pairing_transcript(&self, transcript: &PairingTranscript) -> PairingProof {
        PairingProof {
            public_key: self.public_identity().public_key,
            signature: self.sign_domain_separated(SIGNING_DOMAIN, &transcript.canonical_bytes()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DevicePublicIdentity {
    pub public_key: [u8; 32],
}

impl DevicePublicIdentity {
    pub fn from_public_key(public_key: [u8; 32]) -> Result<Self, IdentityError> {
        VerifyingKey::from_bytes(&public_key).map_err(|_| IdentityError::InvalidPublicKey)?;
        Ok(Self { public_key })
    }

    fn from_verifying_key(key: VerifyingKey) -> Self {
        Self {
            public_key: key.to_bytes(),
        }
    }

    pub fn device_id(&self) -> String {
        device_id_for_public_key(&self.public_key)
    }

    pub fn verify_domain_separated(
        &self,
        domain: &[u8],
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), IdentityError> {
        let key = VerifyingKey::from_bytes(&self.public_key)
            .map_err(|_| IdentityError::InvalidPublicKey)?;
        key.verify(
            &domain_message(domain, message),
            &Signature::from_bytes(signature),
        )
        .map_err(|_| IdentityError::InvalidSignature)
    }
}

pub fn device_id_for_public_key(public_key: &[u8; 32]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ID_DOMAIN);
    hasher.update(public_key);
    BASE32_NOPAD.encode(hasher.finalize().as_bytes())
}

fn domain_message(domain: &[u8], message: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + domain.len() + message.len());
    bytes.extend_from_slice(&(domain.len() as u32).to_be_bytes());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(message);
    bytes
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IdentityError {
    #[error("invalid Ed25519 public key")]
    InvalidPublicKey,
    #[error("signature verification failed")]
    InvalidSignature,
    #[error("participant device ID does not match public key")]
    DeviceIdMismatch,
    #[error("duplicate pairing participant")]
    DuplicateParticipant,
    #[error("pairing proof belongs to an unknown participant")]
    UnknownProofSigner,
    #[error("pairing proofs are incomplete")]
    IncompleteProofs,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PairingRole {
    Initiator,
    Responder,
}

impl PairingRole {
    fn byte(self) -> u8 {
        match self {
            Self::Initiator => 1,
            Self::Responder => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingParticipant {
    pub role: PairingRole,
    pub identity: DevicePublicIdentity,
    pub device_id: String,
    pub certificate_fingerprint: [u8; 32],
    pub nonce: [u8; 32],
}

impl PairingParticipant {
    pub fn new(
        role: PairingRole,
        identity: DevicePublicIdentity,
        certificate_fingerprint: [u8; 32],
        nonce: [u8; 32],
    ) -> Self {
        let device_id = identity.device_id();
        Self {
            role,
            identity,
            device_id,
            certificate_fingerprint,
            nonce,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingTranscript {
    pub protocol_version: u16,
    pub initiator: PairingParticipant,
    pub responder: PairingParticipant,
}

impl PairingTranscript {
    pub fn new(
        initiator: PairingParticipant,
        responder: PairingParticipant,
    ) -> Result<Self, IdentityError> {
        if initiator.role != PairingRole::Initiator
            || responder.role != PairingRole::Responder
            || initiator.identity.public_key == responder.identity.public_key
        {
            return Err(IdentityError::DuplicateParticipant);
        }
        let transcript = Self {
            protocol_version: PAIRING_PROTOCOL_VERSION,
            initiator,
            responder,
        };
        transcript.validate_identities()?;
        Ok(transcript)
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut participants = [&self.initiator, &self.responder];
        participants
            .sort_by(|left, right| left.identity.public_key.cmp(&right.identity.public_key));
        let mut result = Vec::with_capacity(2 + TRANSCRIPT_DOMAIN.len() + 2 * 160);
        result.extend_from_slice(&(TRANSCRIPT_DOMAIN.len() as u16).to_be_bytes());
        result.extend_from_slice(TRANSCRIPT_DOMAIN);
        result.extend_from_slice(&self.protocol_version.to_be_bytes());
        for participant in participants {
            result.push(participant.role.byte());
            result.extend_from_slice(&participant.identity.public_key);
            push_string(&mut result, &participant.device_id);
            result.extend_from_slice(&participant.certificate_fingerprint);
            result.extend_from_slice(&participant.nonce);
        }
        result
    }

    pub fn validate_identities(&self) -> Result<(), IdentityError> {
        for participant in [&self.initiator, &self.responder] {
            if participant.device_id != participant.identity.device_id() {
                return Err(IdentityError::DeviceIdMismatch);
            }
        }
        Ok(())
    }

    pub fn verify_mutual_proofs(&self, proofs: &[PairingProof]) -> Result<(), IdentityError> {
        self.validate_identities()?;
        if proofs.len() != 2 {
            return Err(IdentityError::IncompleteProofs);
        }
        let bytes = self.canonical_bytes();
        let participants = [&self.initiator, &self.responder];
        for participant in participants {
            let proof = proofs
                .iter()
                .find(|proof| proof.public_key == participant.identity.public_key)
                .ok_or(IdentityError::IncompleteProofs)?;
            participant.identity.verify_domain_separated(
                SIGNING_DOMAIN,
                &bytes,
                &proof.signature,
            )?;
        }
        if proofs.iter().any(|proof| {
            !participants
                .iter()
                .any(|participant| participant.identity.public_key == proof.public_key)
        }) {
            return Err(IdentityError::UnknownProofSigner);
        }
        Ok(())
    }

    pub fn sas(&self, proofs: &[PairingProof]) -> Result<SasCode, IdentityError> {
        self.verify_mutual_proofs(proofs)?;
        let mut ordered = proofs.to_vec();
        ordered.sort_by(|left, right| left.public_key.cmp(&right.public_key));
        let mut input = self.canonical_bytes();
        for proof in ordered {
            input.extend_from_slice(&proof.public_key);
            input.extend_from_slice(&proof.signature);
        }
        Ok(SasCode(unbiased_six_digits(&input)))
    }
}

fn push_string(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u32).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingProof {
    pub public_key: [u8; 32],
    pub signature: [u8; 64],
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SasCode(u32);
impl SasCode {
    pub fn value(self) -> u32 {
        self.0
    }
    pub fn formatted(self) -> String {
        format!("{:03} {:03}", self.0 / 1_000, self.0 % 1_000)
    }
}

fn unbiased_six_digits(input: &[u8]) -> u32 {
    let limit = ((1u64 << 32) / 1_000_000) * 1_000_000;
    for counter in 0u32.. {
        let mut hasher = blake3::Hasher::new();
        hasher.update(SAS_DOMAIN);
        hasher.update(input);
        hasher.update(&counter.to_be_bytes());
        let bytes = hasher.finalize();
        for candidate in bytes.as_bytes().chunks_exact(4) {
            let value = u32::from_be_bytes(candidate.try_into().expect("four-byte chunk")) as u64;
            if value < limit {
                return (value % 1_000_000) as u32;
            }
        }
    }
    unreachable!("BLAKE3 retry counter cannot exhaust u32")
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PairingState {
    Pending,
    LocalConfirmed,
    RemoteConfirmed,
    Confirmed,
    Rejected,
    Expired,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PairingError {
    #[error("pairing session has expired")]
    Expired,
    #[error("pairing session was rejected")]
    Rejected,
    #[error("pairing confirmation is invalid in the current state")]
    InvalidState,
}

#[derive(Debug, Clone)]
pub struct PairingSession {
    expires_at: SystemTime,
    state: PairingState,
}

impl PairingSession {
    pub fn new(expires_at: SystemTime) -> Self {
        Self {
            expires_at,
            state: PairingState::Pending,
        }
    }
    pub fn state(&self) -> PairingState {
        self.state
    }
    pub fn confirm_local(&mut self, now: SystemTime) -> Result<PairingState, PairingError> {
        self.confirm(now, true)
    }
    pub fn confirm_remote(&mut self, now: SystemTime) -> Result<PairingState, PairingError> {
        self.confirm(now, false)
    }
    pub fn reject(&mut self) {
        self.state = PairingState::Rejected;
    }
    pub fn expire_if_needed(&mut self, now: SystemTime) -> PairingState {
        if now >= self.expires_at
            && !matches!(self.state, PairingState::Confirmed | PairingState::Rejected)
        {
            self.state = PairingState::Expired;
        }
        self.state
    }
    fn confirm(&mut self, now: SystemTime, local: bool) -> Result<PairingState, PairingError> {
        self.expire_if_needed(now);
        match self.state {
            PairingState::Expired => Err(PairingError::Expired),
            PairingState::Rejected => Err(PairingError::Rejected),
            PairingState::Confirmed => Err(PairingError::InvalidState),
            PairingState::Pending => {
                self.state = if local {
                    PairingState::LocalConfirmed
                } else {
                    PairingState::RemoteConfirmed
                };
                Ok(self.state)
            }
            PairingState::LocalConfirmed if !local => {
                self.state = PairingState::Confirmed;
                Ok(self.state)
            }
            PairingState::RemoteConfirmed if local => {
                self.state = PairingState::Confirmed;
                Ok(self.state)
            }
            _ => Err(PairingError::InvalidState),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};
    fn identity(byte: u8) -> DeviceIdentity {
        DeviceIdentity::from_secret_bytes([byte; 32])
    }
    fn make_transcript() -> (DeviceIdentity, DeviceIdentity, PairingTranscript) {
        let initiator = identity(1);
        let responder = identity(2);
        let transcript = PairingTranscript::new(
            PairingParticipant::new(
                PairingRole::Initiator,
                initiator.public_identity(),
                [3; 32],
                [4; 32],
            ),
            PairingParticipant::new(
                PairingRole::Responder,
                responder.public_identity(),
                [5; 32],
                [6; 32],
            ),
        )
        .unwrap();
        (initiator, responder, transcript)
    }
    #[test]
    fn deterministic_identity_vector() {
        assert_eq!(
            identity(1).public_identity().device_id(),
            "WQY23TGZFUYP3XKMZLELHUNMXNYETCQ7S54VSC5ZO6EFESEIKJ3Q"
        );
    }
    #[test]
    fn secret_export_round_trips_without_debug() {
        let original = identity(7);
        let restored = DeviceIdentity::from_secret_bytes(*original.export_secret_bytes());
        assert_eq!(original.public_identity(), restored.public_identity());
    }
    #[test]
    fn signatures_verify_only_for_exact_domain_and_transcript() {
        let (a, _, transcript) = make_transcript();
        let proof = a.sign_pairing_transcript(&transcript);
        a.public_identity()
            .verify_domain_separated(
                SIGNING_DOMAIN,
                &transcript.canonical_bytes(),
                &proof.signature,
            )
            .unwrap();
        assert_eq!(
            a.public_identity().verify_domain_separated(
                b"wrong",
                &transcript.canonical_bytes(),
                &proof.signature
            ),
            Err(IdentityError::InvalidSignature)
        );
    }
    #[test]
    fn canonical_transcript_is_role_aware_but_key_ordered() {
        let (a, b, original) = make_transcript();
        let reversed = PairingTranscript::new(
            PairingParticipant::new(
                PairingRole::Initiator,
                a.public_identity(),
                [3; 32],
                [4; 32],
            ),
            PairingParticipant::new(
                PairingRole::Responder,
                b.public_identity(),
                [5; 32],
                [6; 32],
            ),
        )
        .unwrap();
        assert_eq!(original.canonical_bytes(), reversed.canonical_bytes());
        let changed_role = PairingTranscript {
            protocol_version: 1,
            initiator: PairingParticipant::new(
                PairingRole::Responder,
                a.public_identity(),
                [3; 32],
                [4; 32],
            ),
            responder: PairingParticipant::new(
                PairingRole::Initiator,
                b.public_identity(),
                [5; 32],
                [6; 32],
            ),
        };
        assert_ne!(original.canonical_bytes(), changed_role.canonical_bytes());
    }
    #[test]
    fn mutual_proofs_and_sas_are_deterministic_vectors() {
        let (a, b, transcript) = make_transcript();
        let proofs = vec![
            a.sign_pairing_transcript(&transcript),
            b.sign_pairing_transcript(&transcript),
        ];
        transcript.verify_mutual_proofs(&proofs).unwrap();
        let sas = transcript.sas(&proofs).unwrap();
        assert_eq!(sas.formatted(), "286 126");
        assert_eq!(sas.value(), 286_126);
    }
    #[test]
    fn tampered_nonce_role_and_signature_are_rejected() {
        let (a, b, mut transcript) = make_transcript();
        let proofs = vec![
            a.sign_pairing_transcript(&transcript),
            b.sign_pairing_transcript(&transcript),
        ];
        transcript.initiator.nonce[0] ^= 1;
        assert_eq!(
            transcript.verify_mutual_proofs(&proofs),
            Err(IdentityError::InvalidSignature)
        );
        let (a, b, transcript) = make_transcript();
        let mut proofs = vec![
            a.sign_pairing_transcript(&transcript),
            b.sign_pairing_transcript(&transcript),
        ];
        proofs[0].signature[0] ^= 1;
        assert_eq!(
            transcript.verify_mutual_proofs(&proofs),
            Err(IdentityError::InvalidSignature)
        );
    }
    #[test]
    fn mismatched_device_id_and_unknown_proof_are_rejected() {
        let (a, b, mut transcript) = make_transcript();
        transcript.initiator.device_id = "wrong".into();
        assert_eq!(
            transcript.verify_mutual_proofs(&[]),
            Err(IdentityError::DeviceIdMismatch)
        );
        let (_, _, transcript) = make_transcript();
        let mut proofs = vec![
            a.sign_pairing_transcript(&transcript),
            b.sign_pairing_transcript(&transcript),
        ];
        proofs[1].public_key = [9; 32];
        assert_eq!(
            transcript.verify_mutual_proofs(&proofs),
            Err(IdentityError::IncompleteProofs)
        );
    }
    #[test]
    fn expiry_rejection_and_confirmation_transitions_are_terminal() {
        let now = UNIX_EPOCH + Duration::from_secs(100);
        let mut session = PairingSession::new(now + Duration::from_secs(120));
        assert_eq!(session.confirm_local(now), Ok(PairingState::LocalConfirmed));
        assert_eq!(session.confirm_remote(now), Ok(PairingState::Confirmed));
        assert_eq!(session.confirm_local(now), Err(PairingError::InvalidState));
        let mut expired = PairingSession::new(now);
        assert_eq!(expired.confirm_local(now), Err(PairingError::Expired));
        let mut rejected = PairingSession::new(now + Duration::from_secs(1));
        rejected.reject();
        assert_eq!(rejected.confirm_remote(now), Err(PairingError::Rejected));
    }
}
