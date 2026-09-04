#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyVerification {
    Known,
    CertificateAuthority,
    Learned,
    Insecure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHostKeyInfo {
    pub algorithm: String,
    pub fingerprint_sha256: String,
    pub verification: HostKeyVerification,
}
