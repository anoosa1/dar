# OIDC and SCIM provider

Delivered November 6, 2026 (Account settings layout B). Dar stays the identity
provider: other applications sign in with Dar over OIDC, and provisioning clients
manage Dar users and groups over SCIM. There is no external identity provider or
outbound provisioning.

Built on `oidc-provider@9.13.3` (with its internal Koa, approved) mounted on the
existing Node server inside Cordis, with a persistent adapter over private,
encrypted SpacetimeDB tables. `openid-client` is used only for tests. Using a
certified library does not make this configuration certified.

## OIDC

- Issuer `<origin>/oidc` with discovery, JWKS (RS256), authorization, token,
  UserInfo, revocation, introspection and RP-initiated logout.
- Clients come from Administration, `services.dar.oidc.clients` or `oidc.json`
  ([deployment](../deployment.md#oidc-clients)). Confidential clients use
  `client_secret_basic`; public clients use `none`. Exact redirect URLs, HTTPS in
  production. No dynamic registration, wildcards or custom URI schemes.
- Authorization code flow only, S256 PKCE required (confidential clients may opt
  out for compatibility, e.g. Gitea). Nonce, state, prompt, max_age, consent and
  account switching. Codes are single-use and atomic.
- Scopes `openid`, `profile`, `email` and configured `groups`. The subject is the
  immutable account ID; email is never marked verified.
- Lifetimes: access and ID tokens 5 minutes, codes 60 seconds, interactions
  10 minutes; grants, provider sessions and rotating refresh tokens at most 7 days
  and bound to the originating native session.
- Logout, password change/recovery, suspension and client disable revoke grants.
  Already-issued ID tokens and relying-party cookies cannot be recalled.
- External signing keys rotate in Administration; retired public keys stay
  published for ten minutes. They are separate from the internal browser issuer.
- Native cookies stay Strict; provider cookies are Lax and Secure. Protocol and
  SCIM routes bypass the browser CSRF guard by design; native decisions keep it.

## SCIM 2.0

- `/scim/v2/Users`, `/Groups`, `/ServiceProviderConfig`, `/Schemas`,
  `/ResourceTypes`, with SCIM media types, errors, ListResponse paging, filters
  (`eq ne co sw ew pr gt ge lt le`, `and/or/not`, value paths), PATCH
  (add/replace/remove), attribute projection and optional `If-Match` (stale → 412).
- Users: `userName`, `externalId`, `displayName`, `name`, `active`, `emails`,
  write-only `password`, `title`, `userType`, `locale`, `timezone`,
  `preferredLanguage`, plus enterprise employee number, cost center,
  organization, division, department and manager. Unknown attributes fail.
- Usernames keep the native 3–64 character format. Deactivation revokes native,
  DAV and provider access immediately. Delete leaves a tombstone, keeps content
  and reserves the ID. The protected operator cannot be renamed, disabled,
  deleted or downgraded by SCIM. Groups never grant admin or app permissions.
- Authentication uses scoped, expiring, revocable provisioning credentials shown
  once and stored as digests; not cookies, DAV passwords or OIDC tokens.

## Administration

`/settings` (also `/identity`): registered applications, directory users and
groups, SCIM credentials and signing keys for administrators; connected
applications for everyone. Sensitive changes need a recent password confirmation
(valid five minutes). APIs live under `/api/v1/identity`.

## Limits and backups

10,000 accounts (including tombstones), 2,000 groups, 2,000 members per group,
200 clients, 50,000 provider records, 200 SCIM resources per page, 256 KiB bodies,
16 KiB user documents, 2,048-character filters (12 levels), 100 PATCH operations.
OIDC routes allow 400 requests/minute per peer and 20 failed confirmations/minute
(behind the local proxy that budget is instance-wide). Backups keep directory,
clients and encrypted keys; restore invalidates codes, sessions, grants and
provisioning credentials. Verify with `node next/scripts/verify.mjs --identity`.

References: [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html) ·
[OAuth security BCP](https://www.rfc-editor.org/rfc/rfc9700) ·
[SCIM schema](https://www.rfc-editor.org/rfc/rfc7643) ·
[SCIM protocol](https://www.rfc-editor.org/rfc/rfc7644)
