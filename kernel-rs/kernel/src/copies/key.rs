use crate::recovery;
use anyhow::{Result, ensure};
use ring::{hkdf, hmac};

pub struct CopyKey {
    files: konae_backup::CopyKey,
    entry_mac: hmac::Key,
    epoch: konae_backup::CopyKey,
    previous: Vec<konae_backup::CopyKey>,
}

impl CopyKey {
    /// Reads the secret as written, untrimmed.
    pub fn from_env() -> Result<Option<Self>> {
        match std::env::var("KONAE_COPY_SECRET").ok().filter(|value| !value.is_empty()) {
            None => Ok(None),
            Some(secret) => {
                ensure!(secret.len() >= 32, "KONAE_COPY_SECRET must be at least 32 characters.");
                let mut key = Self::derive(&secret);
                if let Ok(secret) = std::env::var("KONAE_COPY_V3_SECRET") {
                    key.epoch = key.data_key(&secret)?;
                }
                if let Ok(list) = std::env::var("KONAE_COPY_V3_PREVIOUS_SECRETS") {
                    let secrets = serde_json::from_str::<Vec<String>>(&list)?;
                    key.previous = secrets.iter().map(|s| key.data_key(s)).collect::<Result<_>>()?;
                }
                Ok(Some(key))
            }
        }
    }

    pub fn derive(secret: &str) -> Self {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"konae-copy-v1").extract(secret.as_bytes());
        let entry_mac = prk.expand(&[b"revocation entry mac"], hmac::HMAC_SHA256).expect("HKDF output fits HMAC-SHA256");
        let files = konae_backup::CopyKey::derive(secret);
        let epoch = konae_backup::CopyKey::derive(&format!("v3/{}/{secret}", files.fingerprint()));
        Self { files, epoch, previous: Vec::new(), entry_mac: hmac::Key::from(entry_mac) }
    }

    fn data_key(&self, secret: &str) -> Result<konae_backup::CopyKey> {
        ensure!(secret.len() >= 32, "Each v3 data secret needs at least 32 bytes.");
        Ok(konae_backup::CopyKey::derive(&format!("v3/{}/{secret}", self.fingerprint())))
    }

    pub(super) fn block_keys(&self) -> (&konae_backup::CopyKey, &[konae_backup::CopyKey]) {
        (&self.epoch, &self.previous)
    }

    pub(crate) fn record_mac(&self, repository: &str, name: &str, value: &serde_json::Value) -> Result<String> {
        let message = serde_json::to_vec(&("v3 record", repository, name, value))?;
        Ok(hex::encode(hmac::sign(&self.entry_mac, &message).as_ref()))
    }

    pub(crate) fn check_record_mac(&self, repository: &str, name: &str, bytes: &[u8]) -> Result<()> {
        let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
        let mac = value.as_object_mut().and_then(|o| o.remove("mac")).and_then(|m| m.as_str().map(str::to_owned));
        let message = serde_json::to_vec(&("v3 record", repository, name, &value))?;
        ensure!(mac.and_then(|m| hex::decode(m).ok()).is_some_and(|m| hmac::verify(&self.entry_mac, &message, &m).is_ok()),
            "The v3 revocation record failed authentication.");
        Ok(())
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
