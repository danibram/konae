use crate::CopyKey;
use anyhow::{Context, Result};
use ring::{
    aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM},
    hmac,
    rand::{SecureRandom, SystemRandom},
};

#[derive(Clone)]
pub(super) struct Keys {
    address: hmac::Key,
    block: hmac::Key,
    point: hmac::Key,
}

impl Keys {
    pub fn new(master: &CopyKey, repository: &str) -> Self {
        let derive = |label: &[u8]| {
            let info = [repository.as_bytes(), label];
            hmac::Key::from(
                master
                    .blocks_root
                    .expand(&info, hmac::HMAC_SHA256)
                    .expect("32-byte key"),
            )
        };
        Self {
            address: derive(b"address"),
            block: derive(b"block"),
            point: derive(b"point"),
        }
    }

    pub fn address(&self, plain: &[u8]) -> String {
        hex::encode(hmac::sign(&self.address, plain).as_ref())
    }

    pub fn seal_block(&self, id: &str, plain: Vec<u8>) -> Result<Vec<u8>> {
        // Each address derives a separate AES key for one distinct plaintext.
        seal(&self.block, id, [0; 12], plain)
    }

    pub fn open_block(&self, id: &str, sealed: Vec<u8>) -> Result<Vec<u8>> {
        let plain = open(&self.block, id, [0; 12], sealed)?;
        anyhow::ensure!(
            self.address(&plain) == id,
            "Block address differs from its contents."
        );
        Ok(plain)
    }

    pub fn seal_point(&self, id: &str, plain: Vec<u8>) -> Result<Vec<u8>> {
        let mut nonce = [0; 12];
        SystemRandom::new()
            .fill(&mut nonce)
            .ok()
            .context("Generating a manifest nonce")?;
        let mut bytes = b"KCP3".to_vec();
        bytes.extend_from_slice(&nonce);
        bytes.extend(seal(&self.point, id, nonce, plain)?);
        Ok(bytes)
    }

    pub fn open_point(&self, id: &str, bytes: &[u8]) -> Result<Vec<u8>> {
        anyhow::ensure!(
            bytes.starts_with(b"KCP3") && bytes.len() >= 32,
            "Invalid v3 envelope."
        );
        open(
            &self.point,
            id,
            bytes[4..16].try_into()?,
            bytes[16..].to_vec(),
        )
    }
}

fn key_for(key: &hmac::Key, id: &str) -> LessSafeKey {
    let material = hmac::sign(key, id.as_bytes());
    LessSafeKey::new(UnboundKey::new(&AES_256_GCM, material.as_ref()).expect("32-byte AES key"))
}

fn seal(key: &hmac::Key, id: &str, nonce: [u8; 12], mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    key_for(key, id)
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(id.as_bytes()),
            &mut bytes,
        )
        .ok()
        .context("Encrypting backup data")?;
    Ok(bytes)
}

fn open(key: &hmac::Key, id: &str, nonce: [u8; 12], mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    let length = key_for(key, id)
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(id.as_bytes()),
            &mut bytes,
        )
        .ok()
        .context("Backup authentication failed.")?
        .len();
    bytes.truncate(length);
    Ok(bytes)
}
