//! Tier A capability tokens (hand-written; user-owned; see
//! `metaphor.codegen.yaml`) — ADR-0018, the events file copied in
//! shape, module-local.
//!
//! HMAC-SHA256 over a domain-separated message, base64url-encoded,
//! verified in CONSTANT TIME. The one secret is
//! `BLOG_CAPABILITY_SECRET`; an empty secret is a typed 503 at the
//! visit verb, never a mint under an empty key (fail-closed — a
//! token minted under "" would be forgeable by anyone with the
//! source).
//!
//! Token shape: `v1.<payload-b64url>.<sig-b64url>` where the payload
//! is compact JSON `{purpose, exp, data}` and the signature is
//! HMAC-SHA256(secret, "blog-capability-v1\n" + purpose + "\n" +
//! payload-b64url). Verification recomputes the signature and
//! compares with `subtle` (`ConstantTimeEq`) — never `==`.
//!
//! THE ONE PURPOSE: `blog-view-token` — the visit dedup identity
//! (SPEC section 6 / D4). `data` carries a 43-char random nonce (32
//! random bytes, b64url). There is deliberately NO gated public
//! resource in blog at this version — the view token is the honest
//! Tier A surface, and the family ships ready for the deferred
//! comments/moderation links.
//!
//! Fail-closed on EVERY malformed arm: a token that fails to parse,
//! carries the wrong purpose, the wrong version, a signature
//! mismatch, or an expired `exp` is the SAME uniform refusal
//! (`BlogError::InvalidInput` with a constant message — the caller
//! never learns which arm refused; no oracle).

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::blog_error::BlogError;

/// The view-token purpose (the only purpose at this version).
pub const PURPOSE_VIEW_TOKEN: &str = "blog-view-token";

/// The domain-separation label (first arm of every MAC message) —
/// blog's own, never events'.
const CAPABILITY_CONTEXT: &str = "blog-capability-v1";

type HmacSha256 = Hmac<Sha256>;

/// The env var holding the module's capability secret.
pub const BLOG_CAPABILITY_SECRET_ENV: &str = "BLOG_CAPABILITY_SECRET";

/// View-token lifetime: 180 days.
pub const VIEW_TOKEN_TTL_SECS: i64 = 180 * 24 * 60 * 60;

/// The uniform refusal message for EVERY malformed token arm (the
/// caller never learns which arm refused).
pub const UNIFORM_TOKEN_REFUSAL: &str = "invalid view token";

/// Read the capability secret from the environment (empty string when
/// unset — the visit verb turns that into the typed 503).
pub fn capability_secret_from_env() -> String {
    std::env::var(BLOG_CAPABILITY_SECRET_ENV).unwrap_or_default()
}

fn b64url_encode(bytes: &[u8]) -> String {
    // Standard base64 WITHOUT padding, URL-safe alphabet — the
    // path-segment-safe encoding.
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(ALPHA[(n >> 18) as usize & 63] as char);
        out.push(ALPHA[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHA[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHA[n as usize & 63] as char
        } else {
            '='
        });
    }
    while out.ends_with('=') {
        out.pop();
    }
    out
}

fn b64url_decode(text: &str) -> Option<Vec<u8>> {
    const ALPHA: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut bits: u32 = 0;
    let mut nbits: u32 = 0;
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    for ch in text.bytes() {
        let v = ALPHA.iter().position(|&a| a == ch)? as u32;
        bits = (bits << 6) | v;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push(((bits >> nbits) & 0xff) as u8);
        }
    }
    Some(out)
}

fn sign(secret: &str, purpose: &str, payload_b64: &str) -> Result<Vec<u8>, BlogError> {
    if secret.is_empty() {
        return Err(BlogError::CapabilitySecretNotConfigured);
    }
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| BlogError::Internal(format!("capability secret rejected by HMAC: {e}")))?;
    mac.update(CAPABILITY_CONTEXT.as_bytes());
    mac.update(b"\n");
    mac.update(purpose.as_bytes());
    mac.update(b"\n");
    mac.update(payload_b64.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

/// The token payload. `exp` is unix seconds; `data` is the
/// purpose-scoped scope (the view token carries one random nonce —
/// it identifies a VIEWER SESSION, never a person: no PII, nothing
/// subject to erasure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityClaims {
    pub purpose: String,
    pub exp: i64,
    pub data: Vec<String>,
}

impl CapabilityClaims {
    /// Mint a token for these claims (fails closed on an empty
    /// secret).
    pub fn mint(&self, secret: &str) -> Result<String, BlogError> {
        let payload = serde_json::to_vec(self)
            .map_err(|e| BlogError::Internal(format!("capability payload encode: {e}")))?;
        let payload_b64 = b64url_encode(&payload);
        let sig = sign(secret, &self.purpose, &payload_b64)?;
        Ok(format!("v1.{payload_b64}.{}", b64url_encode(&sig)))
    }

    /// Verify a token against the expected purpose (constant-time
    /// signature compare; expiry checked AFTER the signature so a
    /// forged expiry is not distinguishable from a forged anything).
    pub fn verify(
        secret: &str,
        expected_purpose: &str,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<Self, BlogError> {
        if secret.is_empty() {
            return Err(BlogError::CapabilitySecretNotConfigured);
        }
        let bad = || BlogError::InvalidInput(UNIFORM_TOKEN_REFUSAL.to_string());
        let mut parts = token.splitn(3, '.');
        let version = parts.next().unwrap_or_default();
        let payload_b64 = parts.next().unwrap_or_default();
        let sig_b64 = parts.next().unwrap_or_default();
        if version != "v1" || payload_b64.is_empty() || sig_b64.is_empty() {
            return Err(bad());
        }
        let expected_sig = sign(secret, expected_purpose, payload_b64)?;
        let given_sig = b64url_decode(sig_b64).ok_or_else(bad)?;
        // Constant-time compare (length included): never `==`.
        if expected_sig.len() != given_sig.len() || expected_sig.ct_eq(&given_sig).unwrap_u8() == 0
        {
            return Err(bad());
        }
        let payload = b64url_decode(payload_b64).ok_or_else(bad)?;
        let claims: Self = serde_json::from_slice(&payload).map_err(|_| bad())?;
        if claims.purpose != expected_purpose {
            return Err(bad());
        }
        if now.timestamp() >= claims.exp {
            return Err(bad());
        }
        Ok(claims)
    }
}

/// A fresh random nonce: 32 random bytes, b64url → 43 chars.
pub fn random_nonce() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    b64url_encode(&bytes)
}

/// Mint a view token: one random nonce, TTL from `now`.
pub fn mint_view_token(secret: &str, now: DateTime<Utc>) -> Result<String, BlogError> {
    CapabilityClaims {
        purpose: PURPOSE_VIEW_TOKEN.to_string(),
        exp: now.timestamp() + VIEW_TOKEN_TTL_SECS,
        data: vec![random_nonce()],
    }
    .mint(secret)
}

/// Verify a view token; on success return its nonce (the dedup
/// identity arm). Fails closed on an empty secret.
pub fn verify_view_token(
    secret: &str,
    token: &str,
    now: DateTime<Utc>,
) -> Result<String, BlogError> {
    let claims = CapabilityClaims::verify(secret, PURPOSE_VIEW_TOKEN, token, now)?;
    // A view token with no nonce is malformed — same uniform refusal.
    claims
        .data
        .into_iter()
        .next()
        .ok_or(BlogError::InvalidInput(UNIFORM_TOKEN_REFUSAL.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const SECRET: &str = "test-secret-not-empty";

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn mint_verify_roundtrip() {
        let token = mint_view_token(SECRET, now()).unwrap();
        let nonce = verify_view_token(SECRET, &token, now()).unwrap();
        assert_eq!(nonce.len(), 43);
    }

    #[test]
    fn two_mints_carry_distinct_nonces() {
        let a = mint_view_token(SECRET, now()).unwrap();
        let b = mint_view_token(SECRET, now()).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_purpose_refused() {
        let token = CapabilityClaims {
            purpose: "some-other-purpose".to_string(),
            exp: now().timestamp() + 60,
            data: vec![random_nonce()],
        }
        .mint(SECRET)
        .unwrap();
        let err = verify_view_token(SECRET, &token, now()).unwrap_err();
        assert_eq!(err.code(), "blog_invalid_input");
        // The Display render carries the typed-error prefix; the uniform
        // message itself must ride along unchanged.
        assert!(
            err.to_string().contains(UNIFORM_TOKEN_REFUSAL),
            "refusal message {err} must be the uniform one"
        );
    }

    #[test]
    fn tampered_payload_refused() {
        let token = mint_view_token(SECRET, now()).unwrap();
        let mut parts: Vec<&str> = token.splitn(3, '.').collect();
        parts[1] = "AAAA"; // a forged payload under the real signature
        let err = verify_view_token(SECRET, &parts.join("."), now()).unwrap_err();
        assert_eq!(err.code(), "blog_invalid_input");
    }

    #[test]
    fn expired_refused() {
        // Mint far enough back that mint-time + TTL is already behind
        // the verify clock.
        let minted_at = now() - Duration::seconds(VIEW_TOKEN_TTL_SECS + 10);
        let token = mint_view_token(SECRET, minted_at).unwrap();
        let err = verify_view_token(SECRET, &token, now()).unwrap_err();
        assert_eq!(err.code(), "blog_invalid_input");
    }

    #[test]
    fn empty_secret_never_mints_nor_verifies() {
        let err = mint_view_token("", now()).unwrap_err();
        assert_eq!(err.code(), "blog_capability_secret_not_configured");
        let err = verify_view_token("", "v1.A.B", now()).unwrap_err();
        assert_eq!(err.code(), "blog_capability_secret_not_configured");
    }
}
