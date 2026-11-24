//! RS256 signing keys with rotation (`next/runtime/identity/keys.ts`).
//! Retired public keys stay published for a while so recently issued ID tokens still verify.
//! Persisting and encrypting the keyring is the host's job; `KeyState` is the serializable form.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header};
use rsa::pkcs1::{DecodeRsaPrivateKey, EncodeRsaPrivateKey};
use rsa::traits::PublicKeyParts;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Error, now, random_token};

#[derive(Clone, Serialize, Deserialize)]
pub struct KeyState {
    pub active: StoredKey,
    pub retired: Vec<RetiredKey>,
    /// HMAC key for provider cookies.
    pub cookie_key: String,
    pub rotated_at: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredKey {
    pub kid: String,
    /// PKCS#1 DER, base64url.
    pub private_der: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RetiredKey {
    pub kid: String,
    pub n: String,
    pub e: String,
    pub retire_at: i64,
}

pub struct Keyring {
    state: KeyState,
    encoding: EncodingKey,
    public: (String, String),
}

fn generate() -> Result<StoredKey, Error> {
    let mut rng = getrandom::SysRng;
    let key = rsa::PrivateKey::new(&mut rng, 2048)
        .map_err(|_| Error::Internal("key generation failed"))?;
    let der = key
        .to_pkcs1_der()
        .map_err(|_| Error::Internal("key encoding failed"))?;
    Ok(StoredKey {
        kid: format!("dar-oidc-{}", random_token(16)),
        private_der: B64.encode(der.as_bytes()),
    })
}

fn load(stored: &StoredKey) -> Result<(EncodingKey, (String, String)), Error> {
    let der = B64
        .decode(&stored.private_der)
        .map_err(|_| Error::Internal("invalid stored key"))?;
    let key =
        rsa::PrivateKey::from_pkcs1_der(&der).map_err(|_| Error::Internal("invalid stored key"))?;
    let public = (B64.encode(key.n_bytes()), B64.encode(key.e_bytes()));
    Ok((EncodingKey::from_rsa_der(&der), public))
}

impl Keyring {
    pub fn generate() -> Result<Keyring, Error> {
        let state = KeyState {
            active: generate()?,
            retired: Vec::new(),
            cookie_key: random_token(48),
            rotated_at: now(),
        };
        Keyring::from_state(state)
    }

    pub fn from_state(state: KeyState) -> Result<Keyring, Error> {
        let (encoding, public) = load(&state.active)?;
        Ok(Keyring {
            state,
            encoding,
            public,
        })
    }

    pub fn state(&self) -> &KeyState {
        &self.state
    }

    pub fn cookie_key(&self) -> &[u8] {
        self.state.cookie_key.as_bytes()
    }

    /// Start signing with a new key; the old public key stays published for `keep` seconds.
    pub fn rotate(&mut self, keep: i64) -> Result<(), Error> {
        let at = now();
        let retired = RetiredKey {
            kid: self.state.active.kid.clone(),
            n: self.public.0.clone(),
            e: self.public.1.clone(),
            retire_at: at + keep * 1000,
        };
        let mut state = KeyState {
            active: generate()?,
            retired: vec![retired],
            cookie_key: self.state.cookie_key.clone(),
            rotated_at: at,
        };
        state.retired.extend(
            self.state
                .retired
                .iter()
                .filter(|k| k.retire_at > at)
                .cloned(),
        );
        *self = Keyring::from_state(state)?;
        Ok(())
    }

    /// Drop retired keys past their time. Returns whether anything changed.
    pub fn prune(&mut self) -> bool {
        let at = now();
        let before = self.state.retired.len();
        self.state.retired.retain(|k| k.retire_at > at);
        before != self.state.retired.len()
    }

    pub fn jwks(&self) -> Value {
        let at = now();
        let mut keys = vec![
            json!({"kty": "RSA", "kid": self.state.active.kid, "use": "sig", "alg": "RS256", "n": self.public.0, "e": self.public.1}),
        ];
        keys.extend(self.state.retired.iter().filter(|k| k.retire_at > at).map(|k| {
            json!({"kty": "RSA", "kid": k.kid, "use": "sig", "alg": "RS256", "n": k.n, "e": k.e})
        }));
        json!({ "keys": keys })
    }

    pub fn sign<T: Serialize>(&self, claims: &T) -> Result<String, Error> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.state.active.kid.clone());
        jsonwebtoken::encode(&header, claims, &self.encoding)
            .map_err(|_| Error::Internal("signing failed"))
    }

    /// Verify a token signed by any published key, ignoring expiry (for `id_token_hint`).
    pub fn verify_hint(&self, token: &str, issuer: &str) -> Option<Value> {
        let header = jsonwebtoken::decode_header(token).ok()?;
        let kid = header.kid?;
        let at = now();
        let (n, e) = if kid == self.state.active.kid {
            self.public.clone()
        } else {
            let k = self
                .state
                .retired
                .iter()
                .find(|k| k.kid == kid && k.retire_at > at)?;
            (k.n.clone(), k.e.clone())
        };
        let key = DecodingKey::from_rsa_components(&n, &e).ok()?;
        let mut validation = jsonwebtoken::Validation::new(Algorithm::RS256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        validation.required_spec_claims.clear();
        validation.set_issuer(&[issuer]);
        jsonwebtoken::decode::<Value>(token, &key, &validation)
            .ok()
            .map(|data| data.claims)
    }
}

/// TypeScript's keyring state (`identity/keys.ts`): private and retired keys as JWKs, times in
/// milliseconds. Either runtime reads the other's keyring.
impl Keyring {
    fn jwk_of(stored: &StoredKey) -> Result<Value, Error> {
        use rsa::traits::PrivateKeyParts;
        let der = B64
            .decode(&stored.private_der)
            .map_err(|_| Error::Internal("invalid stored key"))?;
        let key = rsa::PrivateKey::from_pkcs1_der(&der)
            .map_err(|_| Error::Internal("invalid stored key"))?;
        let enc = |v: &rsa::BoxedUint| B64.encode(v.to_be_bytes_trimmed_vartime());
        let primes = key.primes();
        if primes.len() != 2 {
            return Err(Error::Internal("unsupported key"));
        }
        let (p, q) = (primes[0].as_ref(), primes[1].as_ref());
        let mut jwk = json!({ "kty": "RSA", "n": B64.encode(key.n_bytes()), "e": B64.encode(key.e_bytes()), "d": enc(key.d()), "p": enc(p), "q": enc(q), "kid": stored.kid, "alg": "RS256", "use": "sig" });
        if let (Some(dp), Some(dq), Some(qi)) = (key.dp(), key.dq(), key.qinv()) {
            jwk["dp"] = json!(enc(dp));
            jwk["dq"] = json!(enc(dq));
            jwk["qi"] = json!(enc(&qi.retrieve()));
        }
        Ok(jwk)
    }

    fn stored_of(jwk: &Value) -> Result<StoredKey, Error> {
        let part = |k: &str| {
            jwk[k]
                .as_str()
                .and_then(|v| B64.decode(v).ok())
                .map(|b| rsa::BoxedUint::from_be_slice_vartime(&b))
                .ok_or(Error::Internal("invalid key"))
        };
        let key = rsa::PrivateKey::from_p_q(part("p")?, part("q")?, part("e")?)
            .map_err(|_| Error::Internal("invalid key"))?;
        let der = key
            .to_pkcs1_der()
            .map_err(|_| Error::Internal("key encoding failed"))?;
        Ok(StoredKey {
            kid: jwk["kid"].as_str().unwrap_or_default().to_owned(),
            private_der: B64.encode(der.as_bytes()),
        })
    }

    pub fn to_jwk_state(&self) -> Result<Value, Error> {
        let retired: Vec<Value> = self.state.retired.iter().map(|k| json!({ "jwk": { "kty": "RSA", "n": k.n, "e": k.e, "kid": k.kid, "alg": "RS256", "use": "sig" }, "retireAt": k.retire_at })).collect();
        Ok(
            json!({ "active": Self::jwk_of(&self.state.active)?, "retired": retired, "cookieKey": self.state.cookie_key, "rotatedAt": self.state.rotated_at }),
        )
    }

    pub fn from_jwk_state(value: &Value) -> Result<Keyring, Error> {
        let active = Self::stored_of(&value["active"])?;
        let retired = value["retired"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|k| {
                        Some(RetiredKey {
                            kid: k["jwk"]["kid"].as_str()?.into(),
                            n: k["jwk"]["n"].as_str()?.into(),
                            e: k["jwk"]["e"].as_str()?.into(),
                            retire_at: k["retireAt"].as_i64()?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cookie_key = value["cookieKey"]
            .as_str()
            .ok_or(Error::Internal("invalid keyring"))?
            .to_owned();
        Keyring::from_state(KeyState {
            active,
            retired,
            cookie_key,
            rotated_at: value["rotatedAt"].as_i64().unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod jwk_tests {
    use super::*;

    #[test]
    fn jwk_state_round_trips() {
        let keys = Keyring::generate().unwrap();
        let jwk = keys.to_jwk_state().unwrap();
        assert!(jwk["active"]["d"].is_string() && jwk["active"]["qi"].is_string());
        let back = Keyring::from_jwk_state(&jwk).unwrap();
        assert_eq!(back.jwks(), keys.jwks());
        let token = back.sign(&json!({ "iss": "x" })).unwrap();
        assert!(keys.verify_hint(&token, "x").is_some());
    }
}
