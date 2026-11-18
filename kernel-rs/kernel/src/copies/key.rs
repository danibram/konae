use crate::recovery;
use anyhow::{Result, ensure};
use ring::{hkdf, hmac};

pub struct CopyKey {
    files: konae_backup::CopyKey,
    entry_mac: hmac::Key,
}

impl CopyKey {
    /// Reads the secret as written, untrimmed.
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var("KONAE_COPY_SECRET").ok().filter(|value| !value.is_empty()) {
            None => Ok(None),
            Some(secret) => {
                ensure!(secret.len() >= 32, "KONAE_COPY_SECRET must be at least 32 characters.");
                Ok(Some(Self::derive(&secret)))
            }
        }
    }

    pub fn derive(secret: &str) -> Self {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"konae-copy-v1").extract(secret.as_bytes());
        let entry_mac = prk.expand(&[b"revocation entry mac"], hmac::HMAC_SHA256).expect("HKDF output fits HMAC-SHA256");
        Self { files: konae_backup::CopyKey::derive(secret), entry_mac: hmac::Key::from(entry_mac) }
    }

    pub(super) fn files(&self) -> &konae_backup::CopyKey {
        &self.files
    }

    pub(crate) fn sign_entry(&self, name: &str, entry: &recovery::Entry) -> Result<String> {
        Ok(hex::encode(hmac::sign(&self.entry_mac, &entry_message(name, entry)?).as_ref()))
    }

    pub(crate) fn verify_entry(&self, name: &str, entry: &recovery::Entry, mac: &str) -> Result<bool> {
        let message = entry_message(name, entry)?;
        Ok(hex::decode(mac).is_ok_and(|mac| hmac::verify(&self.entry_mac, &message, &mac).is_ok()))
    }
}

impl std::ops::Deref for CopyKey {
    type Target = konae_backup::CopyKey;

    fn deref(&self) -> &Self::Target {
        &self.files
    }
}

fn entry_message(name: &str, entry: &recovery::Entry) -> Result<Vec<u8>> {
    let mut message = name.as_bytes().to_vec();
    message.push(0);
    message.extend(serde_json::to_vec(entry)?);
    Ok(message)
}
