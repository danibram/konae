use crate::{bridge::Bridge, starling::fill_entropy};
use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openssl::{
    bn::{BigNum, BigNumContext, BigNumRef},
    ec::{EcGroup, EcKey, EcPoint, PointConversionForm},
    ecdsa::EcdsaSig,
    hash::{MessageDigest, hash},
    memcmp,
    nid::Nid,
    pkey::{HasPublic, Id, PKey, PKeyRef, Private, Public},
    rsa::{Padding, Rsa, RsaRef},
    sign::{RsaPssSaltlen, Signer, Verifier},
};
use serde::Deserialize;
use serde_json::{Value, json};
use wasmtime::{StoreContextMut, component::Linker};
use zeroize::Zeroizing;

const MAX_KEY_BYTES: usize = 16 * 1024;
const MAX_DATA_BYTES: usize = 16 * 1024 * 1024;
const MAX_KEYS: usize = 64;
const MAX_RSA_BITS: i32 = 8192;
const EC_BYTES: usize = 32;

type CryptoResult<T> = std::result::Result<T, &'static str>;
const DATA_ERROR: &str = "DataError";
const OPERATION_ERROR: &str = "OperationError";
const INVALID_ACCESS: &str = "InvalidAccessError";
const NOT_SUPPORTED: &str = "NotSupportedError";
const SYNTAX_ERROR: &str = "SyntaxError";

#[derive(Clone, Copy, Deserialize, PartialEq)]
enum Algorithm {
    #[serde(rename = "HMAC")]
    Hmac,
    #[serde(rename = "ECDSA")]
    Ecdsa,
    #[serde(rename = "RSASSA-PKCS1-v1_5")]
    RsaPkcs1,
    #[serde(rename = "RSA-PSS")]
    RsaPss,
}

#[derive(Clone, Copy, Deserialize, PartialEq)]
enum Hash {
    #[serde(rename = "SHA-1")]
    Sha1,
    #[serde(rename = "SHA-256")]
    Sha256,
    #[serde(rename = "SHA-384")]
    Sha384,
    #[serde(rename = "SHA-512")]
    Sha512,
}

impl Hash {
    fn parse(name: &str) -> CryptoResult<Self> {
        match name {
            "SHA-1" => Ok(Self::Sha1),
            "SHA-256" => Ok(Self::Sha256),
            "SHA-384" => Ok(Self::Sha384),
            "SHA-512" => Ok(Self::Sha512),
            _ => Err(NOT_SUPPORTED),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "SHA-1",
            Self::Sha256 => "SHA-256",
            Self::Sha384 => "SHA-384",
            Self::Sha512 => "SHA-512",
        }
    }

    fn digest(self) -> MessageDigest {
        match self {
            Self::Sha1 => MessageDigest::sha1(),
            Self::Sha256 => MessageDigest::sha256(),
            Self::Sha384 => MessageDigest::sha384(),
            Self::Sha512 => MessageDigest::sha512(),
        }
    }
}

impl Algorithm {
    fn name(self) -> &'static str {
        match self {
            Self::Hmac => "HMAC",
            Self::Ecdsa => "ECDSA",
            Self::RsaPkcs1 => "RSASSA-PKCS1-v1_5",
            Self::RsaPss => "RSA-PSS",
        }
    }

    fn jwk_alg(self, hash: Option<Hash>) -> Option<String> {
        if self == Self::Ecdsa {
            return Some("ES256".into());
        }
        let prefix = match self {
            Self::Hmac => "HS",
            Self::RsaPkcs1 => "RS",
            Self::RsaPss => "PS",
            Self::Ecdsa => unreachable!(),
        };
        let hash = hash?;
        (hash != Hash::Sha1).then(|| format!("{prefix}{}", &hash.name()[4..]))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportOptions {
    format: String,
    algorithm: Algorithm,
    hash: Option<Hash>,
    named_curve: Option<String>,
    length: Option<u32>,
    extractable: bool,
    usages: Vec<String>,
}

enum KeyMaterial {
    Secret(Zeroizing<Vec<u8>>),
    Private(PKey<Private>),
    Public(PKey<Public>),
}

struct Key {
    material: KeyMaterial,
    algorithm: Algorithm,
    hash: Option<Hash>,
    extractable: bool,
    usages: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Keys(Vec<Key>);

impl Keys {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    fn get(&self, id: u32) -> CryptoResult<&Key> {
        self.0.get(id as usize).ok_or(INVALID_ACCESS)
    }

    fn import(&mut self, options: &str, material: Vec<u8>) -> CryptoResult<String> {
        let material = Zeroizing::new(material);
        if self.0.len() >= MAX_KEYS || options.len() > 1024 || material.len() > MAX_KEY_BYTES {
            return Err(OPERATION_ERROR);
        }
        let options: ImportOptions = serde_json::from_str(options).map_err(|_| NOT_SUPPORTED)?;
        if options.algorithm == Algorithm::Ecdsa {
            if options.named_curve.as_deref() != Some("P-256") || options.hash.is_some() {
                return Err(NOT_SUPPORTED);
            }
        } else if options.hash.is_none() {
            return Err(NOT_SUPPORTED);
        }

        let jwk = if options.format == "jwk" {
            Some(serde_json::from_slice::<Value>(&material).map_err(|_| DATA_ERROR)?)
        } else {
            None
        };
        let mut metadata = json!({ "name": options.algorithm.name() });
        let key = match options.algorithm {
            Algorithm::Hmac => {
                let bytes = match jwk.as_ref() {
                    Some(jwk) if jwk["kty"] == "oct" => jwk_bytes(jwk, "k")?,
                    Some(_) => return Err(DATA_ERROR),
                    None if options.format == "raw" => material,
                    None => return Err(NOT_SUPPORTED),
                };
                let bits = bytes.len() as u32 * 8;
                let length = options.length.unwrap_or(bits);
                if length == 0 || length > bits || bits - length >= 8 {
                    return Err(DATA_ERROR);
                }
                metadata["length"] = json!(length);
                KeyMaterial::Secret(bytes)
            }
            Algorithm::Ecdsa | Algorithm::RsaPkcs1 | Algorithm::RsaPss => {
                let material = import_asymmetric(&options, &material, jwk.as_ref())?;
                match &material {
                    KeyMaterial::Private(key) => describe_key(key, &options, &mut metadata)?,
                    KeyMaterial::Public(key) => describe_key(key, &options, &mut metadata)?,
                    KeyMaterial::Secret(_) => unreachable!(),
                }
                material
            }
        };
        if let Some(hash) = options.hash {
            metadata["hash"] = json!({"name": hash.name()});
        }
        let (kind, allowed): (_, &[_]) = match key {
            KeyMaterial::Secret(_) => ("secret", &["sign", "verify"]),
            KeyMaterial::Private(_) => ("private", &["sign"]),
            KeyMaterial::Public(_) => ("public", &["verify"]),
        };
        if (kind != "public" && options.usages.is_empty())
            || options
                .usages
                .iter()
                .any(|usage| !allowed.contains(&usage.as_str()))
        {
            return Err(SYNTAX_ERROR);
        }
        if let Some(jwk) = jwk.as_ref() {
            validate_jwk(jwk, &options)?;
        }
        let mut usages = Vec::new();
        for usage in options.usages {
            if !usages.contains(&usage) {
                usages.push(usage);
            }
        }
        let info = json!({
            "handle": self.0.len(),
            "type": kind,
            "extractable": options.extractable,
            "usages": usages,
            "algorithm": metadata,
        });
        self.0.push(Key {
            material: key,
            algorithm: options.algorithm,
            hash: options.hash,
            extractable: options.extractable,
            usages,
        });
        Ok(info.to_string())
    }
}

fn validate_jwk(jwk: &Value, options: &ImportOptions) -> CryptoResult<()> {
    if let Some(alg) = jwk.get("alg")
        && (alg.as_str().is_none()
            || alg.as_str() != options.algorithm.jwk_alg(options.hash).as_deref())
    {
        return Err(DATA_ERROR);
    }
    if options.extractable && jwk["ext"] == false {
        return Err(DATA_ERROR);
    }
    if !options.usages.is_empty() && jwk.get("use").is_some_and(|usage| usage != "sig") {
        return Err(DATA_ERROR);
    }
    if let Some(ops) = jwk.get("key_ops") {
        let ops = ops.as_array().ok_or(DATA_ERROR)?;
        if ops
            .iter()
            .enumerate()
            .any(|(index, op)| !op.is_string() || ops[..index].contains(op))
            || options
                .usages
                .iter()
                .any(|usage| !ops.iter().any(|op| op.as_str() == Some(usage)))
        {
            return Err(DATA_ERROR);
        }
    }
    Ok(())
}

fn map_data<T>(result: Result<T, openssl::error::ErrorStack>) -> CryptoResult<T> {
    result.map_err(|_| DATA_ERROR)
}

fn map_operation<T>(result: Result<T, openssl::error::ErrorStack>) -> CryptoResult<T> {
    result.map_err(|_| OPERATION_ERROR)
}

fn jwk_bytes(jwk: &Value, name: &str) -> CryptoResult<Zeroizing<Vec<u8>>> {
    let base64 = jwk[name].as_str().ok_or(DATA_ERROR)?;
    URL_SAFE_NO_PAD
        .decode(base64)
        .map(Zeroizing::new)
        .map_err(|_| DATA_ERROR)
}

fn jwk_int(jwk: &Value, name: &str, max: usize) -> CryptoResult<BigNum> {
    let bytes = jwk_bytes(jwk, name)?;
    if bytes.is_empty() || bytes.len() > max {
        return Err(DATA_ERROR);
    }
    map_data(BigNum::from_slice(&bytes))
}

fn p256() -> CryptoResult<EcGroup> {
    map_operation(EcGroup::from_curve_name(Nid::X9_62_PRIME256V1))
}

fn import_asymmetric(
    options: &ImportOptions,
    bytes: &[u8],
    jwk: Option<&Value>,
) -> CryptoResult<KeyMaterial> {
    match options.format.as_str() {
        "spki" => Ok(KeyMaterial::Public(map_data(PKey::public_key_from_der(
            bytes,
        ))?)),
        "pkcs8" => {
            let key = map_data(PKey::private_key_from_pkcs8(bytes))?;
            if options.algorithm != Algorithm::Ecdsa {
                let rsa = map_data(key.rsa())?;
                check_rsa_private(&rsa)?;
            }
            Ok(KeyMaterial::Private(key))
        }
        "raw" if options.algorithm == Algorithm::Ecdsa => {
            let group = p256()?;
            let mut ctx = map_operation(BigNumContext::new())?;
            let point = map_data(EcPoint::from_bytes(&group, bytes, &mut ctx))?;
            let key = map_data(EcKey::from_public_key(&group, &point))?;
            Ok(KeyMaterial::Public(map_data(PKey::from_ec_key(key))?))
        }
        "jwk" => {
            let jwk = jwk.ok_or(DATA_ERROR)?;
            match options.algorithm {
                Algorithm::Ecdsa => import_ec_jwk(jwk),
                _ => import_rsa_jwk(jwk),
            }
        }
        _ => Err(NOT_SUPPORTED),
    }
}

fn import_ec_jwk(jwk: &Value) -> CryptoResult<KeyMaterial> {
    if jwk["kty"] != "EC" || jwk["crv"] != "P-256" {
        return Err(DATA_ERROR);
    }
    let x = jwk_bytes(jwk, "x")?;
    let y = jwk_bytes(jwk, "y")?;
    if x.len() != EC_BYTES || y.len() != EC_BYTES {
        return Err(DATA_ERROR);
    }
    let group = p256()?;
    let mut ctx = map_operation(BigNumContext::new())?;
    let mut point = map_operation(EcPoint::new(&group))?;
    let x = map_data(BigNum::from_slice(&x))?;
    let y = map_data(BigNum::from_slice(&y))?;
    map_data(point.set_affine_coordinates_gfp(&group, &x, &y, &mut ctx))?;
    if jwk.get("d").is_some() {
        let d = jwk_bytes(jwk, "d")?;
        if d.len() != EC_BYTES {
            return Err(DATA_ERROR);
        }
        let mut d = map_data(BigNum::from_slice(&d))?;
        let key = EcKey::from_private_components(&group, &d, &point);
        d.clear();
        let key = map_data(key)?;
        Ok(KeyMaterial::Private(map_data(PKey::from_ec_key(key))?))
    } else {
        let key = map_data(EcKey::from_public_key(&group, &point))?;
        Ok(KeyMaterial::Public(map_data(PKey::from_ec_key(key))?))
    }
}

fn rsa_bounds(n: &BigNumRef, e: &BigNumRef) -> CryptoResult<()> {
    if n.num_bits() > MAX_RSA_BITS
        || n.is_negative()
        || !n.is_odd()
        || e.is_negative()
        || e.num_bits() > 32
        || e.num_bits() < 2
        || !e.is_odd()
    {
        return Err(DATA_ERROR);
    }
    Ok(())
}

fn check_rsa_private(rsa: &RsaRef<Private>) -> CryptoResult<()> {
    rsa_bounds(rsa.n(), rsa.e())?;
    let mut ctx = map_operation(BigNumContext::new())?;
    let mut modulus = map_operation(BigNum::new())?;
    let p = rsa.p().ok_or(DATA_ERROR)?;
    let q = rsa.q().ok_or(DATA_ERROR)?;
    map_data(modulus.checked_mul(p, q, &mut ctx))?;
    if modulus != *rsa.n() || !map_data(rsa.check_key())? {
        return Err(DATA_ERROR);
    }
    Ok(())
}

fn import_rsa_jwk(jwk: &Value) -> CryptoResult<KeyMaterial> {
    if jwk["kty"] != "RSA" || jwk.get("oth").is_some() {
        return Err(DATA_ERROR);
    }
    let field = |name| jwk_int(jwk, name, MAX_RSA_BITS as usize / 8);
    let n = field("n")?;
    let e = jwk_int(jwk, "e", 4)?;
    rsa_bounds(&n, &e)?;
    if jwk.get("d").is_some() {
        let rsa = map_data(Rsa::from_private_components(
            n,
            e,
            field("d")?,
            field("p")?,
            field("q")?,
            field("dp")?,
            field("dq")?,
            field("qi")?,
        ))?;
        check_rsa_private(&rsa)?;
        Ok(KeyMaterial::Private(map_data(PKey::from_rsa(rsa))?))
    } else {
        let rsa = map_data(Rsa::from_public_components(n, e))?;
        Ok(KeyMaterial::Public(map_data(PKey::from_rsa(rsa))?))
    }
}

fn describe_key<T: HasPublic>(
    key: &PKeyRef<T>,
    options: &ImportOptions,
    metadata: &mut Value,
) -> CryptoResult<()> {
    if options.algorithm == Algorithm::Ecdsa {
        let ec = map_data(key.ec_key())?;
        if key.id() != Id::EC || ec.group().curve_name() != Some(Nid::X9_62_PRIME256V1) {
            return Err(DATA_ERROR);
        }
        map_data(ec.check_key())?;
        metadata["namedCurve"] = json!("P-256");
    } else {
        if key.id() != Id::RSA {
            return Err(DATA_ERROR);
        }
        let rsa = map_data(key.rsa())?;
        rsa_bounds(rsa.n(), rsa.e())?;
        metadata["modulusLength"] = json!(rsa.n().num_bits());
        metadata["publicExponent"] = json!(rsa.e().to_vec());
    }
    Ok(())
}

fn ec_public<T: HasPublic>(key: &PKeyRef<T>) -> CryptoResult<Vec<u8>> {
    let ec = map_operation(key.ec_key())?;
    let mut ctx = map_operation(BigNumContext::new())?;
    map_operation(
        ec.public_key()
            .to_bytes(ec.group(), PointConversionForm::UNCOMPRESSED, &mut ctx),
    )
}

fn public_jwk<T: HasPublic>(key: &PKeyRef<T>, algorithm: Algorithm) -> CryptoResult<Value> {
    if algorithm == Algorithm::Ecdsa {
        let point = ec_public(key)?;
        Ok(json!({
            "kty": "EC", "crv": "P-256",
            "x": URL_SAFE_NO_PAD.encode(&point[1..1 + EC_BYTES]),
            "y": URL_SAFE_NO_PAD.encode(&point[1 + EC_BYTES..]),
        }))
    } else {
        let rsa = map_operation(key.rsa())?;
        Ok(json!({"kty": "RSA", "n": encode_int(rsa.n()), "e": encode_int(rsa.e())}))
    }
}

fn encode_int(value: &BigNumRef) -> String {
    URL_SAFE_NO_PAD.encode(Zeroizing::new(value.to_vec()))
}

impl Key {
    fn export(&self, format: &str) -> CryptoResult<Vec<u8>> {
        if !self.extractable {
            return Err(INVALID_ACCESS);
        }
        match (format, &self.material) {
            ("raw", KeyMaterial::Secret(bytes)) => Ok(bytes.to_vec()),
            ("raw", KeyMaterial::Public(key)) if self.algorithm == Algorithm::Ecdsa => {
                ec_public(key)
            }
            ("spki", KeyMaterial::Public(key)) => map_operation(key.public_key_to_der()),
            ("pkcs8", KeyMaterial::Private(key)) => map_operation(key.private_key_to_pkcs8()),
            ("jwk", _) => {
                let mut jwk = match &self.material {
                    KeyMaterial::Secret(bytes) => {
                        json!({"kty": "oct", "k": URL_SAFE_NO_PAD.encode(bytes.as_slice())})
                    }
                    KeyMaterial::Public(key) => public_jwk(key, self.algorithm)?,
                    KeyMaterial::Private(key) => {
                        let mut jwk = public_jwk(key, self.algorithm)?;
                        if self.algorithm == Algorithm::Ecdsa {
                            let ec = map_operation(key.ec_key())?;
                            let d = map_operation(ec.private_key().to_vec_padded(EC_BYTES as i32))?;
                            jwk["d"] = json!(URL_SAFE_NO_PAD.encode(Zeroizing::new(d)));
                        } else {
                            let rsa = map_operation(key.rsa())?;
                            jwk["d"] = json!(encode_int(rsa.d()));
                            for (field, number) in [
                                ("p", rsa.p()),
                                ("q", rsa.q()),
                                ("dp", rsa.dmp1()),
                                ("dq", rsa.dmq1()),
                                ("qi", rsa.iqmp()),
                            ] {
                                jwk[field] = json!(encode_int(number.ok_or(OPERATION_ERROR)?));
                            }
                        }
                        jwk
                    }
                };
                if let Some(alg) = self.algorithm.jwk_alg(self.hash)
                    && self.algorithm != Algorithm::Ecdsa
                {
                    jwk["alg"] = json!(alg);
                }
                jwk["ext"] = json!(true);
                jwk["key_ops"] = json!(self.usages);
                serde_json::to_vec(&jwk).map_err(|_| OPERATION_ERROR)
            }
            ("raw" | "spki" | "pkcs8", _) => Err(INVALID_ACCESS),
            _ => Err(NOT_SUPPORTED),
        }
    }

    fn operation_hash(
        &self,
        usage: &str,
        algorithm: &str,
        hash: &str,
    ) -> CryptoResult<MessageDigest> {
        if self.algorithm.name() != algorithm || !self.usages.iter().any(|item| item == usage) {
            return Err(INVALID_ACCESS);
        }
        Ok(match self.hash {
            Some(hash) => hash,
            None => Hash::parse(hash)?,
        }
        .digest())
    }

    fn sign(&self, algorithm: &str, hash: &str, salt: u32, data: &[u8]) -> CryptoResult<Vec<u8>> {
        let digest = self.operation_hash("sign", algorithm, hash)?;
        match &self.material {
            KeyMaterial::Secret(bytes) => sign_hmac(bytes, digest, data),
            KeyMaterial::Public(_) => Err(INVALID_ACCESS),
            KeyMaterial::Private(key) => {
                let mut signer = map_operation(Signer::new(digest, key))?;
                if self.algorithm == Algorithm::RsaPss {
                    let salt = pss_salt(key, digest, salt)?;
                    map_operation(signer.set_rsa_padding(Padding::PKCS1_PSS))?;
                    map_operation(signer.set_rsa_pss_saltlen(salt))?;
                    map_operation(signer.set_rsa_mgf1_md(digest))?;
                }
                let signature = map_operation(signer.sign_oneshot_to_vec(data))?;
                if self.algorithm == Algorithm::Ecdsa {
                    let signature = map_operation(EcdsaSig::from_der(&signature))?;
                    let mut raw = map_operation(signature.r().to_vec_padded(EC_BYTES as i32))?;
                    raw.extend(map_operation(signature.s().to_vec_padded(EC_BYTES as i32))?);
                    return Ok(raw);
                }
                Ok(signature)
            }
        }
    }

    fn verify(
        &self,
        algorithm: &str,
        hash: &str,
        salt: u32,
        data: &[u8],
        signature: &[u8],
    ) -> CryptoResult<bool> {
        let digest = self.operation_hash("verify", algorithm, hash)?;
        match &self.material {
            KeyMaterial::Secret(bytes) => {
                let mac = sign_hmac(bytes, digest, data)?;
                Ok(mac.len() == signature.len() && memcmp::eq(&mac, signature))
            }
            KeyMaterial::Private(_) => Err(INVALID_ACCESS),
            KeyMaterial::Public(key) => {
                let der;
                let signature = if self.algorithm == Algorithm::Ecdsa {
                    if signature.len() != 2 * EC_BYTES {
                        return Ok(false);
                    }
                    let r = map_operation(BigNum::from_slice(&signature[..EC_BYTES]))?;
                    let s = map_operation(BigNum::from_slice(&signature[EC_BYTES..]))?;
                    let sig = map_operation(EcdsaSig::from_private_components(r, s))?;
                    der = map_operation(sig.to_der())?;
                    &der
                } else {
                    if signature.len() != key.size() {
                        return Ok(false);
                    }
                    signature
                };
                let mut verifier = map_operation(Verifier::new(digest, key))?;
                if self.algorithm == Algorithm::RsaPss {
                    let Ok(salt) = pss_salt(key, digest, salt) else {
                        return Ok(false);
                    };
                    map_operation(verifier.set_rsa_padding(Padding::PKCS1_PSS))?;
                    map_operation(verifier.set_rsa_pss_saltlen(salt))?;
                    map_operation(verifier.set_rsa_mgf1_md(digest))?;
                }
                map_operation(verifier.verify_oneshot(signature, data))
            }
        }
    }
}

fn pss_salt<T: HasPublic>(
    key: &PKeyRef<T>,
    digest: MessageDigest,
    salt: u32,
) -> CryptoResult<RsaPssSaltlen> {
    let encoded_len = (key.bits() as usize - 1).div_ceil(8);
    if salt as usize + digest.size() + 2 > encoded_len {
        return Err(OPERATION_ERROR);
    }
    Ok(RsaPssSaltlen::custom(salt as i32))
}

fn sign_hmac(bytes: &[u8], digest: MessageDigest, data: &[u8]) -> CryptoResult<Vec<u8>> {
    let key = map_operation(PKey::hmac(bytes))?;
    // OpenSSL's HMAC provider uses the streaming EVP calls.
    let mut signer = map_operation(Signer::new(digest, &key))?;
    map_operation(signer.update(data))?;
    map_operation(signer.sign_to_vec())
}

fn bounded_data(data: &[u8]) -> CryptoResult<()> {
    if data.len() > MAX_DATA_BYTES {
        return Err(OPERATION_ERROR);
    }
    Ok(())
}

fn request<T>(
    store: &mut StoreContextMut<'_, Bridge>,
    run: impl FnOnce(&mut Keys) -> CryptoResult<T>,
) -> Result<(std::result::Result<T, String>,)> {
    store.data().check()?;
    anyhow::ensure!(store.data().request.is_some(), "request capability expired");
    let reply = run(&mut store.data_mut().crypto);
    store.data().check()?;
    Ok((reply.map_err(str::to_owned),))
}

pub(crate) fn link(linker: &mut Linker<Bridge>) -> Result<()> {
    let mut host = linker.instance("konae:worker/crypto")?;
    host.func_wrap("digest", |mut store, (name, data): (String, Vec<u8>)| {
        request(&mut store, |_| {
            bounded_data(&data)?;
            let digest = Hash::parse(&name)?.digest();
            Ok(map_operation(hash(digest, &data))?.to_vec())
        })
    })?;
    host.func_wrap("random-bytes", |mut store, (length,): (u32,)| {
        request(&mut store, |_| {
            if length > 65_536 {
                return Err("QuotaExceededError");
            }
            let mut bytes = vec![0; length as usize];
            fill_entropy(&mut bytes).map_err(|_| OPERATION_ERROR)?;
            Ok(bytes)
        })
    })?;
    host.func_wrap(
        "import-key",
        |mut store, (options, bytes): (String, Vec<u8>)| {
            request(&mut store, |keys| keys.import(&options, bytes))
        },
    )?;
    host.func_wrap("export-key", |mut store, (id, format): (u32, String)| {
        request(&mut store, |keys| keys.get(id)?.export(&format))
    })?;
    host.func_wrap(
        "sign",
        |mut store, (id, algorithm, hash, salt, data): (u32, String, String, u32, Vec<u8>)| {
            request(&mut store, |keys| {
                bounded_data(&data)?;
                keys.get(id)?.sign(&algorithm, &hash, salt, &data)
            })
        },
    )?;
    host.func_wrap(
        "verify",
        |mut store,
         (id, algorithm, hash, salt, data, signature): (
            u32,
            String,
            String,
            u32,
            Vec<u8>,
            Vec<u8>,
        )| {
            request(&mut store, |keys| {
                bounded_data(&data)?;
                keys.get(id)?
                    .verify(&algorithm, &hash, salt, &data, &signature)
            })
        },
    )?;
    Ok(())
}
