//! Passwords, session tokens and the browser token issuer (`runtime/auth/*.ts`), in the
//! same formats: scrypt verifiers, SHA-256 token digests, an AES-GCM sealed ES256 signing
//! key under `secrets/`, CSRF tokens bound to the session secret.

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::error::AppError;

const SCRYPT_PREFIX: &str = "scrypt$131072$8$1";
const MAX_PASSWORD_BYTES: usize = 1024;

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("the system random source is available");
    bytes
}

/// A random lowercase v4 UUID.
pub fn uuid() -> String {
    let mut b = random_bytes::<16>();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

pub fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

/// The SHA-256 of a secret's text, hex (`digestToken`).
pub fn digest_token(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn is_token(secret: &str) -> bool {
    secret.len() == 43
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A 32-byte secret (base64url) and its digest; only the digest is stored.
pub fn opaque_token() -> (String, String) {
    let secret = B64.encode(random_bytes::<32>());
    let digest = digest_token(&secret);
    (secret, digest)
}

pub fn matches_token(secret: &str, digest: &str) -> bool {
    is_token(secret)
        && digest.len() == 64
        && digest_token(secret)
            .as_bytes()
            .ct_eq(digest.as_bytes())
            .into()
}

pub fn validate_new_password(password: &str) -> Result<(), AppError> {
    if password.chars().count() < 15 || password.len() > MAX_PASSWORD_BYTES {
        return Err(AppError::with(
            "invalidPassword",
            400,
            "Use at least 15 characters, up to 1024 UTF-8 bytes.",
        ));
    }
    Ok(())
}

fn derive(password: &str, salt: &[u8]) -> [u8; 32] {
    let params = scrypt::Params::new(17, 8, 1, 32).expect("scrypt parameters");
    let mut out = [0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut out).expect("scrypt output length");
    out
}

/// Hashes the exact password (no normalization); slow by design, so run it off the async runtime.
pub fn hash_password(password: &str) -> String {
    let salt = random_bytes::<16>();
    format!(
        "{SCRYPT_PREFIX}${}${}",
        B64.encode(salt),
        B64.encode(derive(password, &salt))
    )
}

pub fn verify_password(password: &str, verifier: &str) -> bool {
    if password.len() > MAX_PASSWORD_BYTES {
        return false;
    }
    let parts: Vec<&str> = verifier.split('$').collect();
    if parts.len() != 6
        || parts[..4].join("$") != SCRYPT_PREFIX
        || parts[4].len() != 22
        || !is_token(parts[5])
    {
        return false;
    }
    let (Ok(salt), Ok(expected)) = (B64.decode(parts[4]), B64.decode(parts[5])) else {
        return false;
    };
    if B64.encode(&salt) != parts[4] || B64.encode(&expected) != parts[5] {
        return false;
    }
    derive(password, &salt).ct_eq(&expected).into()
}

/// Writes a new private file (0600) and flushes it (`createDurableFile`).
pub fn create_durable_file(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()
}

fn private_text(path: &Path) -> std::io::Result<String> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
        return Err(std::io::Error::other(
            "Identity key files must be regular files private to their owner.",
        ));
    }
    std::fs::read_to_string(path)
}

const KEYRING_AAD: &[u8] = b"dar.identity.signing-key.v1";

fn master_key(directory: &Path, allow_create: bool) -> std::io::Result<[u8; 32]> {
    let path = directory.join("identity-keyring.key");
    let encoded = match private_text(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && allow_create => {
            let text = B64.encode(random_bytes::<32>());
            create_durable_file(&path, &text)?;
            text
        }
        Err(e) => return Err(e),
    };
    let bytes = B64
        .decode(encoded.trim())
        .map_err(|_| std::io::Error::other("Identity keyring is corrupt."))?;
    bytes
        .try_into()
        .map_err(|_| std::io::Error::other("Identity keyring is corrupt."))
}

/// The ES256 key that signs browser tokens and CSRF tokens.
pub struct Issuer {
    pub issuer: String,
    key: SigningKey,
    /// The private scalar as base64url text: the CSRF HMAC key, as in the Node runtime.
    d: String,
    kid: String,
    public: Value,
}

impl Issuer {
    /// Loads `secrets/signing-key.json` (sealed with `identity-keyring.key`), or creates both.
    pub fn open(directory: &Path, issuer: &str) -> std::io::Result<Issuer> {
        let path = directory.join("signing-key.json");
        let jwk: Value = match private_text(&path) {
            Ok(text) => {
                let unsealed = (|| {
                    let envelope: Value = serde_json::from_str(&text).ok()?;
                    if envelope["format"] != "dar-keyring-v1" {
                        return None;
                    }
                    let key = master_key(directory, false).ok()?;
                    let field = |name: &str| B64.decode(envelope[name].as_str()?).ok();
                    let (iv, mut sealed, tag) = (field("iv")?, field("ciphertext")?, field("tag")?);
                    sealed.extend_from_slice(&tag);
                    let plain = Aes256Gcm::new(&key.into())
                        .decrypt(
                            Nonce::from_slice(&iv),
                            Payload {
                                msg: &sealed,
                                aad: KEYRING_AAD,
                            },
                        )
                        .ok()?;
                    serde_json::from_slice(&plain).ok()
                })();
                unsealed.ok_or_else(|| std::io::Error::other("The identity signing key cannot be decrypted. Restore the matching keyring and signing key."))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = master_key(directory, true)?;
                let signing = loop {
                    if let Ok(k) = SigningKey::from_slice(&random_bytes::<32>()) {
                        break k;
                    }
                };
                let point = signing.verifying_key().to_encoded_point(false);
                let jwk = json!({
                    "kty": "EC", "crv": "P-256",
                    "x": B64.encode(point.x().expect("x")), "y": B64.encode(point.y().expect("y")),
                    "d": B64.encode(signing.to_bytes()),
                    "kid": format!("dar-identity-{}", uuid()), "alg": "ES256", "use": "sig",
                });
                let iv = random_bytes::<12>();
                let mut sealed = Aes256Gcm::new(&key.into())
                    .encrypt(
                        Nonce::from_slice(&iv),
                        Payload {
                            msg: jwk.to_string().as_bytes(),
                            aad: KEYRING_AAD,
                        },
                    )
                    .expect("seal");
                let tag = sealed.split_off(sealed.len() - 16);
                let envelope = json!({ "format": "dar-keyring-v1", "iv": B64.encode(iv), "ciphertext": B64.encode(&sealed), "tag": B64.encode(tag) });
                create_durable_file(&path, &envelope.to_string())?;
                jwk
            }
            Err(e) => return Err(e),
        };
        let d = jwk["d"].as_str().unwrap_or_default().to_owned();
        let key = SigningKey::from_slice(
            &B64.decode(&d)
                .map_err(|_| std::io::Error::other("bad key"))?,
        )
        .map_err(|_| std::io::Error::other("bad key"))?;
        let kid = jwk["kid"].as_str().unwrap_or_default().to_owned();
        let mut public = jwk.clone();
        public.as_object_mut().expect("jwk").remove("d");
        Ok(Issuer {
            issuer: issuer.into(),
            key,
            d,
            kid,
            public,
        })
    }

    pub fn jwks(&self) -> Value {
        json!({ "keys": [self.public] })
    }

    /// The CSRF token for a session secret: HMAC-SHA256 keyed by the signing key.
    pub fn csrf(&self, secret: &str) -> String {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(self.d.as_bytes()).expect("HMAC key");
        mac.update(format!("csrf:{secret}").as_bytes());
        B64.encode(mac.finalize().into_bytes())
    }

    /// A browser token for one tab: at most five minutes, never past the session.
    pub fn token(&self, session_id: &str, expires_at: f64, tab: &str) -> Result<String, AppError> {
        if !is_uuid(tab) {
            return Err(AppError::invalid());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as i64;
        let exp = ((expires_at / 1000.0).floor() as i64).min(now + 300);
        let header = json!({ "alg": "ES256", "kid": self.kid });
        let claims = json!({ "sid": session_id, "iss": self.issuer, "aud": "dar-browser", "sub": format!("{session_id}:{tab}"), "iat": now, "exp": exp });
        let input = format!(
            "{}.{}",
            B64.encode(header.to_string()),
            B64.encode(claims.to_string())
        );
        let signature: Signature = self.key.sign(input.as_bytes());
        Ok(format!("{input}.{}", B64.encode(signature.to_bytes())))
    }
}

pub fn secrets_dir(root: &Path) -> PathBuf {
    root.join("secrets")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifiers_match_the_node_format() {
        // Made by next/runtime/auth/password.ts (hashPassword) for "correct horse battery staple".
        let node =
            "scrypt$131072$8$1$OiUG0MlxcNpQ9UHM58-Ucg$d7f6QvMfMZ2anGswucdFkjuCw1uq-_LjmsV2dp7HFB4";
        assert!(
            verify_password("correct horse battery staple", node),
            "Node's verifiers verify here"
        );
        assert!(!verify_password("wrong password, fifteen+", node));
        let mine = hash_password("correct horse battery staple");
        assert!(verify_password("correct horse battery staple", &mine));
        assert!(!verify_password("correct horse battery stapl", &mine));
        assert!(mine.starts_with("scrypt$131072$8$1$"));
    }

    #[test]
    fn tokens_and_csrf() {
        let dir = std::env::temp_dir().join(format!("dar-runtime-test-{}", uuid()));
        let issuer = Issuer::open(&dir, "http://127.0.0.1:1/auth/issuer").unwrap();
        let again = Issuer::open(&dir, "http://127.0.0.1:1/auth/issuer").unwrap();
        assert_eq!(
            issuer.csrf("abc"),
            again.csrf("abc"),
            "the sealed key is reused"
        );
        let token = issuer.token("s", 1e15, &uuid()).unwrap();
        assert_eq!(token.split('.').count(), 3);
        assert!(issuer.token("s", 1e15, "not-a-tab").is_err());
        let (secret, digest) = opaque_token();
        assert!(matches_token(&secret, &digest));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// An app password's grants (`passwordGrants` in shared/app-scopes.ts): legacy write scopes
/// include read; v2 scopes list independent grants.
pub fn password_grants(scope: &str) -> Vec<String> {
    match scope {
        "read" => return vec!["files:read".into()],
        "write" => return vec!["files:read".into(), "files:write".into()],
        _ => {}
    }
    if let Some((app, permission)) = scope.split_once(':')
        && ["contacts", "calendar", "pim"].contains(&app)
        && ["read", "write"].contains(&permission)
    {
        let apps: &[&str] = if app == "pim" {
            &["contacts", "calendar"]
        } else {
            &[app]
        };
        return apps
            .iter()
            .flat_map(|a| {
                if permission == "write" {
                    vec![format!("{a}:read"), format!("{a}:write")]
                } else {
                    vec![format!("{a}:read")]
                }
            })
            .collect();
    }
    scope
        .strip_prefix("v2 ")
        .map(|rest| rest.split(' ').map(str::to_owned).collect())
        .unwrap_or_default()
}

/// A purpose-bound sealed box (`keyringBox`): AES-256-GCM under `identity-keyring.key` with the
/// purpose in the associated data, as `{iv, tag, data}` base64url JSON. Private provider state lives
/// in the database this way without bearer material or client secrets in plaintext.
#[derive(Clone)]
pub struct KeyBox {
    key: [u8; 32],
    aad: Vec<u8>,
}

impl KeyBox {
    pub fn open(secrets: &Path, purpose: &str) -> std::io::Result<KeyBox> {
        Ok(KeyBox {
            key: master_key(secrets, true)?,
            aad: format!("dar.identity.{purpose}.v1").into_bytes(),
        })
    }

    pub fn encrypt(&self, value: &Value) -> String {
        let iv = random_bytes::<12>();
        let sealed = Aes256Gcm::new(&self.key.into())
            .encrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: value.to_string().as_bytes(),
                    aad: &self.aad,
                },
            )
            .expect("seal");
        let (data, tag) = sealed.split_at(sealed.len() - 16);
        json!({ "iv": B64.encode(iv), "tag": B64.encode(tag), "data": B64.encode(data) })
            .to_string()
    }

    pub fn decrypt(&self, encoded: &str) -> Option<Value> {
        let p: Value = serde_json::from_str(encoded).ok()?;
        let part = |k: &str| p[k].as_str().and_then(|v| B64.decode(v).ok());
        let (iv, tag, mut data) = (part("iv")?, part("tag")?, part("data")?);
        if iv.len() != 12 {
            return None;
        }
        data.extend_from_slice(&tag);
        let plain = Aes256Gcm::new(&self.key.into())
            .decrypt(
                Nonce::from_slice(&iv),
                Payload {
                    msg: &data,
                    aad: &self.aad,
                },
            )
            .ok()?;
        serde_json::from_slice(&plain).ok()
    }
}
