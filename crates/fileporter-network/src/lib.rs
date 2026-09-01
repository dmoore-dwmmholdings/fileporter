//! Direct Fileporter TLS session establishment; no discovery or transfer engine.

use std::{sync::Arc, time::Duration};

use fileporter_identity::{
    device_id_for_public_key, DeviceIdentity, DevicePublicIdentity, IdentityError,
};
use fileporter_protocol::{
    decode_frame, encode_control, Auth, ControlMessage, Frame, Hello, ProtocolError, ALPN,
    PROTOCOL_PREFACE, PROTOCOL_VERSION,
};
use rand_core::{OsRng, RngCore};
use rcgen::{CertificateParams, CustomExtension, KeyPair, PKCS_ED25519};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{ring, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::{
        danger::{ClientCertVerified, ClientCertVerifier},
        WebPkiClientVerifier,
    },
    ClientConfig, DigitallySignedStruct, Error as RustlsError, RootCertStore, ServerConfig,
    SignatureScheme,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_rustls::{
    client::TlsStream as ClientTlsStream, server::TlsStream as ServerTlsStream, TlsAcceptor,
    TlsConnector,
};
use x509_parser::{oid_registry::Oid, prelude::parse_x509_certificate};

pub const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
const BINDING_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 57264, 1, 1];
const BINDING_DOMAIN: &[u8] = b"fileporter/tls-binding/v1";
const AUTH_DOMAIN: &[u8] = b"fileporter/session-auth/v1";

#[derive(Debug)]
pub enum NetworkError {
    Io(std::io::Error),
    Tls(RustlsError),
    Protocol(ProtocolError),
    Identity(IdentityError),
    Certificate(String),
    Timeout,
    PinMismatch,
    ProtocolMismatch,
    UnexpectedMessage,
}
impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Io(_) => "network I/O failed",
                Self::Tls(_) => "TLS authentication failed",
                Self::Protocol(_) => "protocol framing failed",
                Self::Identity(_) => "identity verification failed",
                Self::Certificate(_) => "certificate binding failed",
                Self::Timeout => "session timed out",
                Self::PinMismatch => "trusted peer pin mismatch",
                Self::ProtocolMismatch => "protocol version mismatch",
                Self::UnexpectedMessage => "unexpected session message",
            }
        )
    }
}
impl std::error::Error for NetworkError {}
impl From<std::io::Error> for NetworkError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<RustlsError> for NetworkError {
    fn from(value: RustlsError) -> Self {
        Self::Tls(value)
    }
}
impl From<ProtocolError> for NetworkError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}
impl From<IdentityError> for NetworkError {
    fn from(value: IdentityError) -> Self {
        Self::Identity(value)
    }
}

pub struct LocalCertificate {
    certificate: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    binding: PeerBinding,
    identity: DeviceIdentity,
}
impl LocalCertificate {
    pub fn generate(identity: &DeviceIdentity) -> Result<Self, NetworkError> {
        let key = KeyPair::generate_for(&PKCS_ED25519)
            .map_err(|error| NetworkError::Certificate(error.to_string()))?;
        let tls_public = key.public_key_raw();
        let identity_public = identity.public_identity();
        let binding_message = binding_message(&identity_public.public_key, tls_public);
        let signature = identity.sign_domain_separated(BINDING_DOMAIN, &binding_message);
        let mut extension = Vec::with_capacity(96);
        extension.extend_from_slice(&identity_public.public_key);
        extension.extend_from_slice(&signature);
        let mut params = CertificateParams::new(vec!["fileporter.local".into()])
            .map_err(|error| NetworkError::Certificate(error.to_string()))?;
        params
            .custom_extensions
            .push(CustomExtension::from_oid_content(BINDING_OID, extension));
        let certificate = params
            .self_signed(&key)
            .map_err(|error| NetworkError::Certificate(error.to_string()))?;
        let der = certificate.der().clone();
        let binding = parse_binding(&der)?;
        Ok(Self {
            certificate: der,
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            binding,
            identity: DeviceIdentity::from_secret_bytes(*identity.export_secret_bytes()),
        })
    }
    pub fn certificate_der(&self) -> CertificateDer<'static> {
        self.certificate.clone()
    }
    /// Restores a locally generated certificate and its PKCS#8 private key.
    /// The caller is responsible for keeping `private_key_der` in a secret store.
    pub fn from_persisted_der(
        identity: &DeviceIdentity,
        certificate_der: Vec<u8>,
        private_key_der: Vec<u8>,
    ) -> Result<Self, NetworkError> {
        let certificate = CertificateDer::from(certificate_der);
        let binding = parse_binding(&certificate)?;
        if binding.identity != identity.public_identity() {
            return Err(NetworkError::Certificate(
                "certificate belongs to another identity".into(),
            ));
        }
        let key = KeyPair::try_from(private_key_der.as_slice())
            .map_err(|error| NetworkError::Certificate(error.to_string()))?;
        let (_, parsed) = parse_x509_certificate(certificate.as_ref())
            .map_err(|_| NetworkError::Certificate("invalid certificate".into()))?;
        if parsed.tbs_certificate.subject_pki.subject_public_key.data != key.public_key_raw() {
            return Err(NetworkError::Certificate(
                "private key does not match certificate".into(),
            ));
        }
        Ok(Self {
            certificate,
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der)),
            binding,
            identity: DeviceIdentity::from_secret_bytes(*identity.export_secret_bytes()),
        })
    }
    /// Returns credential bytes only for persistence by the platform secret-store adapter.
    pub fn persisted_der(&self) -> (Vec<u8>, Vec<u8>) {
        (
            self.certificate.as_ref().to_vec(),
            self.key.secret_der().to_vec(),
        )
    }
    pub fn fingerprint(&self) -> [u8; 32] {
        certificate_fingerprint(&self.certificate)
    }
    pub fn binding(&self) -> &PeerBinding {
        &self.binding
    }
    /// Returns an ephemeral signing adapter for the pairing transcript.  The
    /// secret never crosses a network or command boundary.
    pub fn identity_for_pairing(&self) -> DeviceIdentity {
        DeviceIdentity::from_secret_bytes(*self.identity.export_secret_bytes())
    }
    fn key_clone(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerBinding {
    pub identity: DevicePublicIdentity,
    pub device_id: String,
    pub certificate_fingerprint: [u8; 32],
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedPeerPin {
    pub device_id: String,
    pub public_key: [u8; 32],
    pub certificate_fingerprint: [u8; 32],
}
impl TrustedPeerPin {
    pub fn from_binding(binding: &PeerBinding) -> Self {
        Self {
            device_id: binding.device_id.clone(),
            public_key: binding.identity.public_key,
            certificate_fingerprint: binding.certificate_fingerprint,
        }
    }
    fn matches(&self, binding: &PeerBinding) -> bool {
        self.device_id == binding.device_id
            && self.public_key == binding.identity.public_key
            && self.certificate_fingerprint == binding.certificate_fingerprint
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustMode {
    Pairing,
    Trusted(TrustedPeerPin),
}
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SessionAuthorization {
    PairingOnly,
    Trusted,
}
#[derive(Debug, Clone)]
pub struct AuthenticatedSession {
    pub peer: PeerBinding,
    pub authorization: SessionAuthorization,
}

pub fn certificate_fingerprint(certificate: &CertificateDer<'_>) -> [u8; 32] {
    *blake3::hash(certificate.as_ref()).as_bytes()
}

fn binding_message(identity: &[u8; 32], tls_public: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(BINDING_DOMAIN.len() + identity.len() + tls_public.len());
    bytes.extend_from_slice(&(BINDING_DOMAIN.len() as u16).to_be_bytes());
    bytes.extend_from_slice(BINDING_DOMAIN);
    bytes.extend_from_slice(identity);
    bytes.extend_from_slice(&(tls_public.len() as u16).to_be_bytes());
    bytes.extend_from_slice(tls_public);
    bytes
}
fn parse_binding(certificate: &CertificateDer<'_>) -> Result<PeerBinding, NetworkError> {
    let (_, parsed) = parse_x509_certificate(certificate.as_ref())
        .map_err(|_| NetworkError::Certificate("invalid DER".into()))?;
    let oid = Oid::from(BINDING_OID)
        .map_err(|_| NetworkError::Certificate("invalid binding OID".into()))?;
    let extension = parsed
        .extensions()
        .iter()
        .find(|extension| extension.oid == oid)
        .ok_or_else(|| NetworkError::Certificate("missing identity binding".into()))?;
    if extension.value.len() != 96 {
        return Err(NetworkError::Certificate(
            "invalid identity binding length".into(),
        ));
    }
    let public_key: [u8; 32] = extension.value[..32].try_into().expect("checked length");
    let signature: [u8; 64] = extension.value[32..].try_into().expect("checked length");
    let identity = verify_binding_signature(
        public_key,
        signature,
        &parsed.tbs_certificate.subject_pki.subject_public_key.data,
    )?;
    Ok(PeerBinding {
        device_id: device_id_for_public_key(&public_key),
        identity,
        certificate_fingerprint: certificate_fingerprint(certificate),
    })
}

fn verify_binding_signature(
    public_key: [u8; 32],
    signature: [u8; 64],
    tls_public: &[u8],
) -> Result<DevicePublicIdentity, NetworkError> {
    let identity = DevicePublicIdentity::from_public_key(public_key)?;
    identity.verify_domain_separated(
        BINDING_DOMAIN,
        &binding_message(&public_key, tls_public),
        &signature,
    )?;
    Ok(identity)
}

#[derive(Debug)]
struct BindingVerifier {
    mode: TrustMode,
}
/// Only used while a peer is inside the pairing state machine.  It still
/// requires a Fileporter identity-bound certificate and a valid TLS proof of
/// possession; it deliberately does not turn off certificate verification.
#[derive(Debug)]
struct PairingClientVerifier;
impl ClientCertVerifier for PairingClientVerifier {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, RustlsError> {
        parse_binding(end_entity)
            .map_err(|_| RustlsError::General("invalid Fileporter identity binding".into()))?;
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
impl BindingVerifier {
    fn verify_binding(&self, cert: &CertificateDer<'_>) -> Result<(), RustlsError> {
        let binding = parse_binding(cert)
            .map_err(|_| RustlsError::General("invalid Fileporter identity binding".into()))?;
        if let TrustMode::Trusted(pin) = &self.mode {
            if !pin.matches(&binding) {
                return Err(RustlsError::General("Fileporter peer pin mismatch".into()));
            }
        }
        Ok(())
    }
}
impl ServerCertVerifier for BindingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        self.verify_binding(end_entity)?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn server_config_for_client(
    local: &LocalCertificate,
    accepted_client: &TrustedPeerPin,
    client_certificate: CertificateDer<'static>,
) -> Result<Arc<ServerConfig>, NetworkError> {
    let mut roots = RootCertStore::empty();
    roots
        .add(client_certificate)
        .map_err(|error| NetworkError::Certificate(error.to_string()))?;
    let provider = Arc::new(ring::default_provider());
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|error| NetworkError::Certificate(error.to_string()))?;
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![local.certificate_der()], local.key_clone())
        .map_err(NetworkError::Tls)?;
    config.alpn_protocols = vec![ALPN.as_bytes().to_vec()];
    let _ = accepted_client; // Pin is checked after TLS from the presented binding by the caller's session policy.
    Ok(Arc::new(config))
}

/// Server configuration for the narrowly-scoped, explicit pairing protocol.
/// Callers must reject every frame except pairing frames before trust commits.
pub fn pairing_server_config(local: &LocalCertificate) -> Result<Arc<ServerConfig>, NetworkError> {
    let provider = Arc::new(ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(Arc::new(PairingClientVerifier))
        .with_single_cert(vec![local.certificate_der()], local.key_clone())
        .map_err(NetworkError::Tls)?;
    config.alpn_protocols = vec![ALPN.as_bytes().to_vec()];
    Ok(Arc::new(config))
}
pub fn client_config(
    local: &LocalCertificate,
    mode: TrustMode,
) -> Result<Arc<ClientConfig>, NetworkError> {
    let verifier = Arc::new(BindingVerifier { mode });
    let mut config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(vec![local.certificate_der()], local.key_clone())
        .map_err(NetworkError::Tls)?;
    config.alpn_protocols = vec![ALPN.as_bytes().to_vec()];
    Ok(Arc::new(config))
}

pub async fn connect_authenticated(
    address: std::net::SocketAddr,
    local: &LocalCertificate,
    mode: TrustMode,
) -> Result<(ClientTlsStream<TcpStream>, AuthenticatedSession), NetworkError> {
    let stream = timeout(SESSION_TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    let connector = TlsConnector::from(client_config(local, mode.clone())?);
    let server_name = ServerName::try_from("fileporter.local")
        .map_err(|_| NetworkError::Certificate("invalid server name".into()))?
        .to_owned();
    let mut tls = timeout(SESSION_TIMEOUT, connector.connect(server_name, stream))
        .await
        .map_err(|_| NetworkError::Timeout)?
        .map_err(tls_handshake_error)?;
    let peer_cert = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .ok_or_else(|| NetworkError::Certificate("missing peer certificate".into()))?;
    let peer = parse_binding(peer_cert)?;
    let authorization = match mode {
        TrustMode::Pairing => SessionAuthorization::PairingOnly,
        TrustMode::Trusted(pin) if pin.matches(&peer) => SessionAuthorization::Trusted,
        TrustMode::Trusted(_) => return Err(NetworkError::PinMismatch),
    };
    authenticate(&mut tls, local, &peer).await?;
    Ok((
        tls,
        AuthenticatedSession {
            peer,
            authorization,
        },
    ))
}

fn tls_handshake_error(error: std::io::Error) -> NetworkError {
    match error.kind() {
        std::io::ErrorKind::InvalidData
        | std::io::ErrorKind::InvalidInput
        | std::io::ErrorKind::Unsupported => {
            NetworkError::Tls(RustlsError::General(error.to_string()))
        }
        _ => NetworkError::Io(error),
    }
}

pub async fn accept_authenticated(
    listener: &TcpListener,
    acceptor: TlsAcceptor,
    local: &LocalCertificate,
    expected_client: TrustedPeerPin,
) -> Result<(ServerTlsStream<TcpStream>, AuthenticatedSession), NetworkError> {
    let (stream, _) = timeout(SESSION_TIMEOUT, listener.accept())
        .await
        .map_err(|_| NetworkError::Timeout)??;
    let mut tls = timeout(SESSION_TIMEOUT, acceptor.accept(stream))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    let peer_cert = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .ok_or_else(|| NetworkError::Certificate("missing client certificate".into()))?;
    let peer = parse_binding(peer_cert)?;
    if !expected_client.matches(&peer) {
        return Err(NetworkError::PinMismatch);
    }
    authenticate(&mut tls, local, &peer).await?;
    Ok((
        tls,
        AuthenticatedSession {
            peer,
            authorization: SessionAuthorization::Trusted,
        },
    ))
}

/// Accept an unknown-but-identity-bound peer for pairing only.  The returned
/// authorization must never be used to authorize a transfer.
pub async fn accept_pairing_authenticated(
    listener: &TcpListener,
    acceptor: TlsAcceptor,
    local: &LocalCertificate,
) -> Result<(ServerTlsStream<TcpStream>, AuthenticatedSession), NetworkError> {
    let (stream, _) = timeout(SESSION_TIMEOUT, listener.accept())
        .await
        .map_err(|_| NetworkError::Timeout)??;
    accept_pairing_stream_authenticated(stream, acceptor, local).await
}

/// Pairing-only equivalent of [`accept_pairing_authenticated`] for a TCP
/// stream which has already been admitted by a long-lived listener.
pub async fn accept_pairing_stream_authenticated(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    local: &LocalCertificate,
) -> Result<(ServerTlsStream<TcpStream>, AuthenticatedSession), NetworkError> {
    let mut tls = timeout(SESSION_TIMEOUT, acceptor.accept(stream))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    let peer_cert = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certificates| certificates.first())
        .ok_or_else(|| NetworkError::Certificate("missing client certificate".into()))?;
    let peer = parse_binding(peer_cert)?;
    authenticate(&mut tls, local, &peer).await?;
    Ok((
        tls,
        AuthenticatedSession {
            peer,
            authorization: SessionAuthorization::PairingOnly,
        },
    ))
}

/// Accept a stream with the permissive pairing certificate verifier, then let
/// the application bind the authenticated identity to a durable trusted-peer
/// pin before it permits any transfer frames.  This is intentionally separate
/// from [`accept_authenticated`]: a long-lived listener cannot know which
/// pinned peer is connecting until it has inspected the identity binding.
pub async fn accept_identity_authenticated_stream(
    stream: TcpStream,
    acceptor: TlsAcceptor,
    local: &LocalCertificate,
) -> Result<(ServerTlsStream<TcpStream>, AuthenticatedSession), NetworkError> {
    accept_pairing_stream_authenticated(stream, acceptor, local).await
}

async fn authenticate<S>(
    stream: &mut S,
    local: &LocalCertificate,
    peer: &PeerBinding,
) -> Result<(), NetworkError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    timeout(SESSION_TIMEOUT, stream.write_all(&PROTOCOL_PREFACE))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    timeout(SESSION_TIMEOUT, stream.flush())
        .await
        .map_err(|_| NetworkError::Timeout)??;
    let mut received_preface = [0u8; 13];
    timeout(SESSION_TIMEOUT, stream.read_exact(&mut received_preface))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    if received_preface != PROTOCOL_PREFACE {
        return Err(NetworkError::ProtocolMismatch);
    }
    let mut nonce = [0u8; 32];
    OsRng.fill_bytes(&mut nonce);
    let local_id = local.binding().identity.clone();
    let hello = ControlMessage::Hello(Hello {
        protocol_min: PROTOCOL_VERSION,
        protocol_max: PROTOCOL_VERSION,
        device_id: local.binding().device_id.clone(),
        display_name: "Fileporter".into(),
        session_nonce: hex::encode(nonce),
        capabilities: vec!["pairing".into(), "transfer".into()],
    });
    send_control(stream, &hello).await?;
    let remote_hello = receive_control(stream).await?;
    let remote_nonce: [u8; 32] = match remote_hello {
        ControlMessage::Hello(hello)
            if hello.protocol_min <= PROTOCOL_VERSION
                && hello.protocol_max >= PROTOCOL_VERSION
                && hello.device_id == peer.device_id =>
        {
            hex::decode(hello.session_nonce)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(NetworkError::ProtocolMismatch)?
        }
        _ => return Err(NetworkError::ProtocolMismatch),
    };
    let transcript = session_transcript(
        &local_id.public_key,
        &peer.identity.public_key,
        &nonce,
        &remote_nonce,
    );
    let auth = ControlMessage::Auth(Auth {
        transcript: hex::encode(&transcript),
        signature: hex::encode(
            local_identity_from_binding(local)?.sign_domain_separated(AUTH_DOMAIN, &transcript),
        ),
    });
    send_control(stream, &auth).await?;
    let remote_auth = receive_control(stream).await?;
    match remote_auth {
        ControlMessage::Auth(auth) if auth.transcript == hex::encode(&transcript) => {
            let signature: [u8; 64] = hex::decode(auth.signature)
                .ok()
                .and_then(|value| value.try_into().ok())
                .ok_or(NetworkError::ProtocolMismatch)?;
            peer.identity
                .verify_domain_separated(AUTH_DOMAIN, &transcript, &signature)?;
        }
        _ => return Err(NetworkError::UnexpectedMessage),
    }
    Ok(())
}

// Certificate creation currently keeps no identity key; session authentication receives it through this short-lived adapter.
fn local_identity_from_binding(local: &LocalCertificate) -> Result<DeviceIdentity, NetworkError> {
    Ok(DeviceIdentity::from_secret_bytes(
        *local.identity.export_secret_bytes(),
    ))
}
fn session_transcript(
    left: &[u8; 32],
    right: &[u8; 32],
    left_nonce: &[u8; 32],
    right_nonce: &[u8; 32],
) -> Vec<u8> {
    let mut records = [(left, left_nonce), (right, right_nonce)];
    records.sort_by(|a, b| a.0.cmp(b.0));
    let mut result = b"fileporter/session-auth-transcript/v1".to_vec();
    for (key, nonce) in records {
        result.extend_from_slice(key);
        result.extend_from_slice(nonce);
    }
    result
}
/// Sends one complete Fileporter frame over an authenticated stream.  Callers
/// must only use this after `connect_authenticated`/`accept_authenticated` and
/// must enforce their own protocol phase.
pub async fn send_frame<S: AsyncWrite + Unpin>(
    stream: &mut S,
    frame: &Frame,
) -> Result<(), NetworkError> {
    let encoded = match frame {
        Frame::Control(message) => encode_control(message)?,
        Frame::Chunk(chunk) => fileporter_protocol::encode_chunk(chunk)?,
    };
    timeout(SESSION_TIMEOUT, stream.write_all(&encoded))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    timeout(SESSION_TIMEOUT, stream.flush())
        .await
        .map_err(|_| NetworkError::Timeout)??;
    Ok(())
}

/// Receives exactly one bounded Fileporter frame from an authenticated stream.
/// The length is checked before allocation so a peer cannot force an
/// unbounded allocation.
pub async fn receive_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Frame, NetworkError> {
    let mut header = [0u8; 5];
    timeout(SESSION_TIMEOUT, stream.read_exact(&mut header))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    let length = u32::from_be_bytes(header[1..].try_into().expect("frame header")) as usize;
    let limit = match header[0] {
        1 => fileporter_protocol::MAX_CONTROL_PAYLOAD,
        2 => fileporter_protocol::MAX_CHUNK_DATA + 76,
        kind => {
            return Err(NetworkError::Protocol(ProtocolError::UnknownFrameKind(
                kind,
            )))
        }
    };
    if length > limit {
        return Err(NetworkError::Protocol(ProtocolError::OversizedFrame {
            actual: length,
            limit,
        }));
    }
    let mut encoded = header.to_vec();
    encoded.resize(5 + length, 0);
    timeout(SESSION_TIMEOUT, stream.read_exact(&mut encoded[5..]))
        .await
        .map_err(|_| NetworkError::Timeout)??;
    decode_frame(&encoded).map_err(Into::into)
}

async fn send_control<S: AsyncWrite + Unpin>(
    stream: &mut S,
    message: &ControlMessage,
) -> Result<(), NetworkError> {
    send_frame(stream, &Frame::Control(message.clone())).await
}
async fn receive_control<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<ControlMessage, NetworkError> {
    match receive_frame(stream).await? {
        Frame::Control(control) => Ok(control),
        Frame::Chunk(_) => Err(NetworkError::UnexpectedMessage),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn local(seed: u8) -> LocalCertificate {
        LocalCertificate::generate(&DeviceIdentity::from_secret_bytes([seed; 32])).unwrap()
    }

    async fn server_for(
        server: &LocalCertificate,
        client: &LocalCertificate,
    ) -> (TcpListener, TlsAcceptor, TrustedPeerPin) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client_pin = TrustedPeerPin::from_binding(client.binding());
        let config =
            server_config_for_client(server, &client_pin, client.certificate_der()).unwrap();
        (listener, TlsAcceptor::from(config), client_pin)
    }

    #[tokio::test]
    async fn trusted_loopback_session_authenticates_both_peers() {
        let server = local(1);
        let client = local(2);
        let (listener, acceptor, client_pin) = server_for(&server, &client).await;
        let address = listener.local_addr().unwrap();
        let server_pin = TrustedPeerPin::from_binding(server.binding());
        let (server_result, client_result) = tokio::join!(
            accept_authenticated(&listener, acceptor, &server, client_pin),
            connect_authenticated(address, &client, TrustMode::Trusted(server_pin))
        );
        assert_eq!(
            client_result.unwrap().1.authorization,
            SessionAuthorization::Trusted
        );
        assert_eq!(
            server_result.unwrap().1.authorization,
            SessionAuthorization::Trusted
        );
    }

    #[tokio::test]
    async fn pairing_returns_identity_without_transfer_authorization() {
        let server = local(3);
        let client = local(4);
        let (listener, acceptor, client_pin) = server_for(&server, &client).await;
        let address = listener.local_addr().unwrap();
        let (server_result, client_result) = tokio::join!(
            accept_authenticated(&listener, acceptor, &server, client_pin),
            connect_authenticated(address, &client, TrustMode::Pairing)
        );
        assert_eq!(
            client_result.unwrap().1.authorization,
            SessionAuthorization::PairingOnly
        );
        assert_eq!(
            server_result.unwrap().1.authorization,
            SessionAuthorization::Trusted
        );
    }

    #[tokio::test]
    async fn wrong_trusted_pin_is_rejected() {
        let server = local(5);
        let client = local(6);
        let (listener, acceptor, client_pin) = server_for(&server, &client).await;
        let address = listener.local_addr().unwrap();
        let wrong = TrustedPeerPin::from_binding(client.binding());
        let (server_result, client_result) = tokio::join!(
            accept_authenticated(&listener, acceptor, &server, client_pin),
            connect_authenticated(address, &client, TrustMode::Trusted(wrong))
        );
        assert!(matches!(
            client_result,
            Err(NetworkError::Tls(_)) | Err(NetworkError::PinMismatch)
        ));
        assert!(server_result.is_err());
    }

    #[test]
    fn certificate_binding_tampering_is_rejected() {
        let certificate = local(7);
        let der = certificate.certificate_der();
        let (_, parsed) = parse_x509_certificate(der.as_ref()).unwrap();
        let oid = Oid::from(BINDING_OID).unwrap();
        let extension = parsed
            .extensions()
            .iter()
            .find(|extension| extension.oid == oid)
            .unwrap();
        let public_key: [u8; 32] = extension.value[..32].try_into().unwrap();
        let mut signature: [u8; 64] = extension.value[32..].try_into().unwrap();
        signature[0] ^= 1;
        assert!(verify_binding_signature(
            public_key,
            signature,
            &parsed.tbs_certificate.subject_pki.subject_public_key.data
        )
        .is_err());
        let mut pin = TrustedPeerPin::from_binding(certificate.binding());
        pin.certificate_fingerprint[0] ^= 1;
        assert!(!pin.matches(certificate.binding()));
    }

    #[tokio::test]
    async fn protocol_preface_mismatch_is_rejected() {
        let certificate = local(8);
        let peer = certificate.binding().clone();
        let (mut remote, mut local_stream) = tokio::io::duplex(128);
        let remote_task = tokio::spawn(async move {
            let mut received = [0u8; 13];
            remote.read_exact(&mut received).await.unwrap();
            remote.write_all(b"NOTFILEPORTER!").await.unwrap();
        });
        let result = authenticate(&mut local_stream, &certificate, &peer).await;
        remote_task.await.unwrap();
        assert!(matches!(result, Err(NetworkError::ProtocolMismatch)));
    }
}
