use crate::Manifest;
use anyhow::Result;
use ring::{
    aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
    hkdf, hmac,
    rand::{SecureRandom, SystemRandom},
};

/// Seals files and signs manifests with separate keys derived from the same secret.
pub struct CopyKey {
    pub(crate) blocks_root: hkdf::Prk,
    key: LessSafeKey,
    check: String,
    manifest_mac: hmac::Key,
}

impl CopyKey {
    /// The `keyCheck` a sealed manifest carries. It tells two secrets apart
    /// without revealing either.
    pub fn fingerprint(&self) -> &str {
        &self.check
    }

    pub fn derive(secret: &str) -> Self {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, b"konae-copy-v1").extract(secret.as_bytes());
        let okm = prk.expand(&[b"copy file"], &AES_256_GCM).expect("HKDF output fits AES-256");
        let key = LessSafeKey::new(UnboundKey::from(okm));
        let mut check = [0u8; 32];
        prk.expand(&[b"key check"], HkdfLength(32)).expect("HKDF output fits 32 bytes").fill(&mut check).expect("32 bytes");
        let manifest_mac = prk.expand(&[b"manifest mac"], hmac::HMAC_SHA256).expect("HKDF output fits HMAC-SHA256");
        let blocks_root = hkdf::Salt::new(hkdf::HKDF_SHA256, b"konae-copy-v3").extract(secret.as_bytes());
        Self { blocks_root, key, check: hex::encode(check), manifest_mac: hmac::Key::from(manifest_mac) }
    }

    pub(crate) fn sign_manifest(&self, copy: &str, manifest: &Manifest) -> String {
        hex::encode(hmac::sign(&self.manifest_mac, &manifest_message(copy, manifest)).as_ref())
    }

    pub fn verify_manifest(&self, copy: &str, manifest: &Manifest, mac: &str) -> bool {
        hex::decode(mac).is_ok_and(|mac| hmac::verify(&self.manifest_mac, &manifest_message(copy, manifest), &mac).is_ok())
    }

    /// Seals one file. The copy ID and the file's path are the associated
    /// data, so a sealed file moved to another copy or path does not open.
    /// The output is the nonce followed by the ciphertext and its tag.
    pub fn seal(&self, copy: &str, path: &str, mut bytes: Vec<u8>) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new().fill(&mut nonce).map_err(|_| anyhow::anyhow!("Could not generate a nonce."))?;
        self.key
            .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(format!("{copy}/{path}").as_bytes()), &mut bytes)
            .map_err(|_| anyhow::anyhow!("Could not encrypt {path}."))?;
        let mut sealed = nonce.to_vec();
        sealed.extend_from_slice(&bytes);
        Ok(sealed)
    }

    pub fn open(&self, copy: &str, path: &str, sealed: &[u8]) -> Option<Vec<u8>> {
        if sealed.len() < NONCE_LEN {
            return None;
        }
        let (nonce, body) = sealed.split_at(NONCE_LEN);
        let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
        let mut buffer = body.to_vec();
        let plain = self.key.open_in_place(nonce, Aad::from(format!("{copy}/{path}").as_bytes()), &mut buffer).ok()?;
        Some(plain.to_vec())
    }
}

fn manifest_message(copy: &str, manifest: &Manifest) -> Vec<u8> {
    let mut message = copy.as_bytes().to_vec();
    message.push(0);
    let plain = Manifest { mac: None, ..manifest.clone() };
    message.extend(serde_json::to_vec(&plain).expect("a manifest serialises"));
    message
}

struct HkdfLength(usize);

impl hkdf::KeyType for HkdfLength {
    fn len(&self) -> usize {
        self.0
    }
}
