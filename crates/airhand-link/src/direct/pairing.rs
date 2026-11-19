//! Pairing transcript rules. Transport authentication and session consent are
//! separate: consuming an invitation never grants input or screen access.
use super::Secret;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const VERSION: u16 = 1;
pub const TTL: u64 = 600;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub v: u16,
    pub ctrl_node_pub: String,
    pub ctrl_tls_fp: [u8; 32],
    pub commitment: [u8; 32],
    pub req_id: [u8; 16],
    pub expires: u64,
    pub name: String,
}
impl Request {
    pub fn validate(&self, now: u64) -> Result<()> {
        ensure!(
            self.v == VERSION && self.expires > now && self.expires <= now.saturating_add(TTL),
            "invalid or expired pairing request"
        );
        let public = self.ctrl_node_pub.strip_prefix("nodekey:").unwrap_or("");
        ensure!(
            public.len() == 64
                && public.bytes().all(|b| b.is_ascii_hexdigit())
                && public.bytes().any(|b| b != b'0'),
            "invalid pairing identity"
        );
        ensure!(
            self.ctrl_tls_fp != [0; 32] && self.commitment != [0; 32] && self.req_id != [0; 16],
            "invalid pairing identity"
        );
        ensure!(
            !self.name.is_empty()
                && self.name.len() <= 128
                && !self.name.chars().any(char::is_control),
            "invalid untrusted device label"
        );
        Ok(())
    }
    pub fn verify_reveal(&self, nonce: &Secret<[u8; 32]>) -> Result<()> {
        ensure!(
            hash(nonce.expose()) == self.commitment,
            "pairing commitment mismatch"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    pub v: u16,
    pub tailcat_addr: String,
    pub host_tls_fp: [u8; 32],
    pub nh: [u8; 32],
    pub request_hash: [u8; 32],
    pub expires: u64,
}
impl Invitation {
    pub fn validate(&self, request: &Request, request_bytes: &[u8], now: u64) -> Result<()> {
        request.validate(now)?;
        ensure!(
            self.v == VERSION && self.expires > now && self.expires <= request.expires,
            "invalid or expired invitation"
        );
        ensure!(
            self.request_hash == hash(request_bytes),
            "invitation does not bind the pending request"
        );
        ensure!(
            self.host_tls_fp != [0; 32]
                && self.nh != [0; 32]
                && self.tailcat_addr.starts_with("tc")
                && self.tailcat_addr.len() <= 16384,
            "invalid invitation identity"
        );
        Ok(())
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct PendingRequest {
    pub request: Request,
    pub consumed: bool,
}
impl PendingRequest {
    pub fn consume(
        &mut self,
        invitation: &Invitation,
        request_bytes: &[u8],
        now: u64,
    ) -> Result<()> {
        ensure!(!self.consumed, "invitation already consumed");
        invitation.validate(&self.request, request_bytes, now)?;
        self.consumed = true;
        Ok(())
    }
}
pub fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub fn sas(nc: &Secret<[u8; 32]>, nh: &[u8; 32], request: &[u8], invitation: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(nc.expose());
    digest.update(nh);
    digest.update(request);
    digest.update(invitation);
    let value: [u8; 32] = digest.finalize().into();
    format!(
        "{:06}",
        u32::from_be_bytes(value[..4].try_into().unwrap()) % 1_000_000
    )
}
#[derive(Serialize, Deserialize)]
pub struct DirectHello {
    pub v: u16,
    pub nc: [u8; 32],
}
#[derive(Serialize, Deserialize)]
pub struct DirectReady {
    pub v: u16,
    pub input: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> Request {
        Request {
            v: 1,
            ctrl_node_pub: format!("nodekey:{}", "ab".repeat(32)),
            ctrl_tls_fp: [1; 32],
            commitment: hash(&[7; 32]),
            req_id: [2; 16],
            expires: 700,
            name: "Untrusted label".into(),
        }
    }
    fn invitation() -> Invitation {
        Invitation {
            v: 1,
            tailcat_addr: "tc-example".into(),
            host_tls_fp: [3; 32],
            nh: [4; 32],
            request_hash: hash(b"request"),
            expires: 700,
        }
    }
    #[test]
    fn consumes_once_and_rejects_unknown_request() {
        let mut pending = PendingRequest {
            request: request(),
            consumed: false,
        };
        let i = invitation();
        assert!(pending.consume(&i, b"foreign", 100).is_err());
        assert!(!pending.consumed);
        pending.consume(&i, b"request", 100).unwrap();
        assert!(pending.consume(&i, b"request", 100).is_err());
    }
    #[test]
    fn expiry_tampering_and_commitment_fail_closed() {
        let r = request();
        let i = invitation();
        assert!(i.validate(&r, b"request", 700).is_err());
        assert!(i.validate(&r, b"tampered", 100).is_err());
        assert!(r.verify_reveal(&Secret::new([8; 32])).is_err());
        r.verify_reveal(&Secret::new([7; 32])).unwrap();
        let mut r = r.clone();
        r.expires = 1000;
        assert!(r.validate(100).is_err());
    }
    #[test]
    fn sas_binds_both_raw_transcripts_and_nonces() {
        let n = Secret::new([7; 32]);
        let code = sas(&n, &[4; 32], b"request", b"invitation");
        assert_eq!(code.len(), 6);
        assert_ne!(code, sas(&n, &[4; 32], b"tampered", b"invitation"));
        assert_ne!(code, sas(&n, &[5; 32], b"request", b"invitation"));
    }
}
