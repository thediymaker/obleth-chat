//! The installation's secret key (OBLETH_CHAT_SECRET_KEY_FILE), which encrypts what
//! Chat keeps for people: connection tokens and identity provider tokens. Values are
//! sealed with XChaCha20-Poly1305 and bound to the row they belong to, so a sealed
//! value copied to another row does not open.

use std::{path::Path, sync::Arc};

use anyhow::{bail, Context};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;

/// A 32-byte key. It has no Debug or Display, so it cannot reach a log.
#[derive(Clone)]
pub struct SecretKey(Arc<[u8; 32]>);

impl SecretKey {
    /// Reads a key file holding 32 raw bytes, 64 hex characters, or base64 of 32 bytes.
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path).context("Cannot read OBLETH_CHAT_SECRET_KEY_FILE")?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        if let Ok(raw) = <[u8; 32]>::try_from(bytes) {
            return Ok(Self::from_bytes(raw));
        }
        let text = std::str::from_utf8(bytes).unwrap_or("").trim();
        if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut key = [0u8; 32];
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16)?;
            }
            return Ok(Self::from_bytes(key));
        }
        if let Some(key) = STANDARD
            .decode(text)
            .ok()
            .and_then(|key| <[u8; 32]>::try_from(key).ok())
        {
            return Ok(Self::from_bytes(key));
        }
        bail!("OBLETH_CHAT_SECRET_KEY_FILE must hold 32 bytes: raw, as 64 hex characters, or as base64");
    }

    pub fn from_bytes(key: [u8; 32]) -> Self {
        Self(Arc::new(key))
    }

    pub(crate) fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encrypts `plain` bound to `aad`, prefixed with a one-byte format `version`.
    pub fn seal(&self, version: u8, aad: &[u8], plain: &[u8]) -> Vec<u8> {
        seal_with(self.bytes(), version, aad, plain)
    }

    /// Decrypts a value sealed with this key and format `version`.
    pub fn open(&self, version: u8, aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        open_with(self.bytes(), version, aad, sealed)
    }
}

pub(crate) fn seal_with(key: &[u8; 32], version: u8, aad: &[u8], plain: &[u8]) -> Vec<u8> {
    let mut nonce = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut nonce);
    let sealed = XChaCha20Poly1305::new(key.into())
        .encrypt(&XNonce::from(nonce), Payload { msg: plain, aad })
        .expect("encryption with a valid key cannot fail");
    [&[version][..], &nonce, &sealed].concat()
}

pub(crate) fn open_with(key: &[u8; 32], version: u8, aad: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let (found, rest) = sealed.split_first()?;
    if *found != version || rest.len() < 24 {
        return None;
    }
    let (nonce, body) = rest.split_at(24);
    let nonce: [u8; 24] = nonce.try_into().ok()?;
    XChaCha20Poly1305::new(key.into())
        .decrypt(&XNonce::from(nonce), Payload { msg: body, aad })
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_files_accept_raw_hex_and_base64() {
        let raw = [7u8; 32];
        let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        let b64 = STANDARD.encode(raw);
        for form in [
            raw.to_vec(),
            format!("{hex}\n").into_bytes(),
            b64.into_bytes(),
        ] {
            assert_eq!(SecretKey::parse(&form).unwrap().bytes(), &raw);
        }
        for bad in [&b"short"[..], &[1u8; 31], b"zz".repeat(32).as_slice()] {
            assert!(SecretKey::parse(bad).is_err());
        }
    }

    #[test]
    fn sealed_values_open_only_with_their_binding_and_version() {
        let key = SecretKey::from_bytes([3u8; 32]);
        let sealed = key.seal(2, b"row-a", b"token");
        assert_eq!(key.open(2, b"row-a", &sealed).unwrap(), b"token");
        assert!(key.open(2, b"row-b", &sealed).is_none());
        assert!(key.open(1, b"row-a", &sealed).is_none());
        assert!(SecretKey::from_bytes([4u8; 32])
            .open(2, b"row-a", &sealed)
            .is_none());
    }
}
