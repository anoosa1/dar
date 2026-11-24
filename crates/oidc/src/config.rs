//! Provider settings and client registration, validated like Dar's `clientMetadata`
//! (`next/runtime/identity/http.ts`).

use std::collections::BTreeSet;

use url::Url;

use crate::Error;

/// Scopes Dar understands, with the labels shown on the consent screen.
pub const SCOPES: [(&str, &str); 5] = [
    ("openid", "Identify your account"),
    ("profile", "Read your name and username"),
    ("email", "Read your email address"),
    ("groups", "Read your directory group IDs"),
    (
        "offline_access",
        "Keep access with a refresh token while your Dar session remains active",
    ),
];

pub fn scope_label(scope: &str) -> &str {
    SCOPES
        .iter()
        .find(|(s, _)| *s == scope)
        .map_or(scope, |(_, label)| label)
}

/// Lifetimes in seconds (`ttl` in `provider.ts`).
#[derive(Clone, Debug)]
pub struct Lifetimes {
    pub code: i64,
    pub access_token: i64,
    pub id_token: i64,
    pub interaction: i64,
    /// Grants, provider sessions and refresh tokens.
    pub session: i64,
    /// Retired signing keys stay published this long.
    pub retired_key: i64,
}

impl Default for Lifetimes {
    fn default() -> Self {
        Self {
            code: 60,
            access_token: 300,
            id_token: 300,
            interaction: 600,
            session: 7 * 86400,
            retired_key: 600,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    /// Dar's origin, for example `https://dar.example`. The issuer is `<origin>/oidc`.
    pub origin: Url,
    /// Development allows loopback HTTP redirects and drops the Secure cookie flag.
    pub development: bool,
    pub lifetimes: Lifetimes,
}

impl ProviderConfig {
    pub fn new(origin: &str, development: bool) -> Result<Self, Error> {
        let origin = Url::parse(origin)
            .map_err(|_| Error::Config("origin must be an absolute URL".into()))?;
        if origin.path() != "/" || origin.query().is_some() || origin.fragment().is_some() {
            return Err(Error::Config(
                "origin must not have a path, query or fragment".into(),
            ));
        }
        Ok(Self {
            origin,
            development,
            lifetimes: Lifetimes::default(),
        })
    }
    pub fn issuer(&self) -> String {
        format!("{}/oidc", self.origin.as_str().trim_end_matches('/'))
    }
    pub fn interaction_url(&self, uid: &str) -> String {
        format!(
            "{}/identity/interaction/{uid}",
            self.origin.as_str().trim_end_matches('/')
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientKind {
    /// `token_endpoint_auth_method: none`; PKCE is always required.
    Public,
    /// `client_secret_basic`. Only the SHA-256 of the secret is kept.
    Confidential { secret_digest: [u8; 32] },
}

#[derive(Clone, Debug)]
pub struct Client {
    pub id: String,
    pub name: String,
    pub kind: ClientKind,
    pub redirect_uris: Vec<String>,
    pub post_logout_redirect_uris: Vec<String>,
    pub scopes: BTreeSet<String>,
    pub require_pkce: bool,
    pub enabled: bool,
    /// Built-in clients (the Immich app) are not administered and may use their custom scheme.
    pub built_in: bool,
    /// Extra consent lines shown for built-in clients.
    pub permissions: Vec<(String, String)>,
}

impl Client {
    pub fn allows_refresh(&self) -> bool {
        self.scopes.contains("offline_access")
    }
}

/// What an administrator or `oidc.json` supplies for one client.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInput {
    pub id: String,
    pub name: String,
    /// `"public"` or `"confidential"`.
    pub kind: String,
    pub secret: Option<String>,
    pub redirect_uris: Vec<String>,
    pub post_logout_redirect_uris: Vec<String>,
    pub scopes: Vec<String>,
    pub require_pkce: Option<bool>,
    pub enabled: Option<bool>,
}

fn redirects(values: &[String], development: bool, optional: bool) -> Result<Vec<String>, Error> {
    let limit = if optional { 10 } else { 20 };
    if values.len() > limit || (!optional && values.is_empty()) {
        return Err(Error::InvalidClient(
            "redirect URL list is empty or too long".into(),
        ));
    }
    let mut out: Vec<String> = Vec::new();
    for raw in values {
        let parsed = Url::parse(raw)
            .map_err(|_| Error::InvalidClient("redirect URLs must be absolute".into()))?;
        let loopback = matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        let scheme_ok =
            parsed.scheme() == "https" || (development && parsed.scheme() == "http" && loopback);
        if raw.len() > 2048
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some()
            || parsed.as_str() != raw
            || raw.contains('*')
            || !scheme_ok
        {
            return Err(Error::InvalidClient(
                "Use exact HTTPS redirect URLs; loopback HTTP is allowed only in development."
                    .into(),
            ));
        }
        if !out.contains(raw) {
            out.push(raw.clone());
        }
    }
    Ok(out)
}

pub fn valid_client_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && id.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

impl Client {
    /// Validate administrator or host input.
    pub fn from_input(input: &ClientInput, development: bool) -> Result<Client, Error> {
        if !valid_client_id(&input.id) {
            return Err(Error::InvalidClient(
                "client IDs must be URL-safe identifiers".into(),
            ));
        }
        let name = input.name.trim();
        if name.is_empty() || name.len() > 100 {
            return Err(Error::InvalidClient(
                "client name is required (up to 100 bytes)".into(),
            ));
        }
        let kind = match (input.kind.as_str(), &input.secret) {
            ("public", None) => ClientKind::Public,
            ("public", Some(_)) => {
                return Err(Error::InvalidClient(
                    "public clients cannot have a secret".into(),
                ));
            }
            ("confidential", Some(secret))
                if secret.len() >= 32 && !secret.contains(['\r', '\n']) =>
            {
                ClientKind::Confidential {
                    secret_digest: crate::digest(secret.as_bytes()),
                }
            }
            ("confidential", _) => {
                return Err(Error::InvalidClient(
                    "client secrets need at least 32 characters and no newlines".into(),
                ));
            }
            _ => {
                return Err(Error::InvalidClient(
                    "client type must be public or confidential".into(),
                ));
            }
        };
        if kind == ClientKind::Public && input.require_pkce == Some(false) {
            return Err(Error::InvalidClient(
                "PKCE can only be disabled for confidential clients.".into(),
            ));
        }
        let scopes: BTreeSet<String> = input
            .scopes
            .iter()
            .flat_map(|s| s.split_whitespace())
            .map(str::to_owned)
            .collect();
        if !scopes.contains("openid")
            || scopes
                .iter()
                .any(|s| !SCOPES.iter().any(|(known, _)| known == s))
        {
            return Err(Error::InvalidClient(
                "scopes must include openid and only Dar scopes".into(),
            ));
        }
        Ok(Client {
            id: input.id.clone(),
            name: name.to_owned(),
            kind,
            redirect_uris: redirects(&input.redirect_uris, development, false)?,
            post_logout_redirect_uris: redirects(
                &input.post_logout_redirect_uris,
                development,
                true,
            )?,
            scopes,
            require_pkce: input.require_pkce != Some(false),
            enabled: input.enabled != Some(false),
            built_in: false,
            permissions: Vec::new(),
        })
    }

    /// The Immich mobile app's "Sign in with Dar" (`immichOAuthClient` in `shared/photos.ts`).
    pub fn immich() -> Client {
        Client {
            id: "immich-app".into(),
            name: "Immich".into(),
            kind: ClientKind::Public,
            redirect_uris: vec!["app.immich:///oauth-callback".into()],
            post_logout_redirect_uris: Vec::new(),
            scopes: BTreeSet::from(["openid".to_owned()]),
            require_pkce: true,
            enabled: true,
            built_in: true,
            permissions: vec![(
                "photos".into(),
                "Back up, browse and change your photos (a Photos app password for this device)"
                    .into(),
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ClientInput {
        ClientInput {
            id: "gitea".into(),
            name: "Gitea".into(),
            kind: "confidential".into(),
            secret: Some("s".repeat(40)),
            redirect_uris: vec!["https://git.example/callback".into()],
            scopes: vec!["openid profile".into()],
            ..Default::default()
        }
    }

    #[test]
    fn accepts_exact_https_redirects() {
        let client = Client::from_input(&input(), false).unwrap();
        assert!(client.require_pkce);
        assert_eq!(client.scopes.len(), 2);
    }

    #[test]
    fn rejects_loose_redirects() {
        for uri in [
            "http://git.example/cb",
            "https://git.example/cb#x",
            "https://*.example/cb",
            "https://GIT.example/cb",
            "https://u:p@git.example/cb",
        ] {
            let mut bad = input();
            bad.redirect_uris = vec![uri.into()];
            assert!(Client::from_input(&bad, false).is_err(), "{uri}");
        }
        let mut dev = input();
        dev.redirect_uris = vec!["http://127.0.0.1:8080/cb".into()];
        assert!(Client::from_input(&dev, false).is_err());
        assert!(Client::from_input(&dev, true).is_ok());
    }

    #[test]
    fn public_clients_keep_pkce() {
        let mut public = input();
        public.kind = "public".into();
        public.secret = None;
        public.require_pkce = Some(false);
        assert!(Client::from_input(&public, false).is_err());
        let mut short = input();
        short.secret = Some("short".into());
        assert!(Client::from_input(&short, false).is_err());
    }
}
