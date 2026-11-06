# Native OIDC and SCIM provider

Requested November 6, 2026, after native auth, Files, Contacts and Calendar.
Branch: `codex/spacetimedb-auth-files`. Status: concrete implementation proposal;
dependency approval and static UI selection pending. Protocol implementation has
not started, and the app is not yet a general OIDC or SCIM provider.

## Provider boundary

Dar remains the identity provider and owns passwords, login and its directory.
Other applications use Dar for OIDC sign-in; provisioning clients manage Dar's
users/groups through SCIM. This does not add an external identity provider,
outbound SCIM connectors or an alternative account database.

Use `oidc-provider@9.13.3`, MIT-licensed, with `@types/oidc-provider@9.13.1`.
Its provider callback mounts on the existing Node HTTP server inside the Cordis
lifecycle. The package uses Koa internally; this proposal explicitly requests
that transitive dependency boundary exception. Do not start another listener,
runtime, service or database. Native React interactions replace the package's
development UI. `openid-client`, already approved in the parent dependency list,
provides independent relying-party integration tests.

This is a new dependency and needs the maintainer's approval under AGENTS.md.
Do not install it, change the package lock, or add its runtime code beforehand.
Using a certified library does not mean this Dar configuration is certified.

## OIDC functionality

- Publish a dedicated external issuer at `/oidc` with discovery, JWKS,
  authorization, token, UserInfo, revocation, introspection and RP-initiated logout.
  Advertise only features actually configured and tested.
- Administer registered public and confidential clients; exact redirect and
  post-logout URI allowlists, client authentication, allowed scopes, enable/disable,
  secret rotation and deletion. No open registration endpoint or wildcard URIs.
  Production clients use HTTPS; development exceptions are explicit loopback URIs.
- Authorization code flow with required S256 PKCE. Support nonce, state passthrough,
  prompt/reauthentication, max_age, consent approval/denial and account switching.
  No password, implicit or hybrid grant. Persist codes and consume them atomically;
  replay, expiry, wrong verifier and redirect/client mismatches must fail.
- Offer `openid`, `profile`, `email` and explicitly configured group claims.
  The subject is the immutable account ID, unaffected by username changes.
  Do not infer email verification from a supplied email address. Claims follow
  scopes and consent and never expose credential or application-content tables.
- Refresh-token rotation, expiry, family replay handling and grant revocation.
  Users can list/revoke connected applications. Disabled clients or users cannot
  continue token exchange, refresh or UserInfo access despite cached metadata.
- Reuse the native account/password verifier and session checks. Interaction POSTs
  need exact-origin validation, CSRF protection, bounded requests and rate limits.
  Do not weaken existing Strict native cookies to implement cross-site OIDC.
  Provider-purpose cookies have their own deliberate SameSite policy.
- Native logout, password recovery/change and suspension invalidate associated
  provider sessions and grants at the owning transaction boundary. OIDC logout
  validates client/logout destinations. Previously issued signed ID tokens and
  independent relying-party cookies cannot be remotely erased; document that
  limit instead of promising instant logout of every external application.
- External OIDC signing keys use a separate encrypted keyring and rotation/retirement
  lifecycle. Keep the internal `/auth/issuer` SDK identity stable and separate:
  its audience is `dar-browser` and its per-tab subject must never become an
  external application's ID token. Publish retained public keys long enough to
  verify unexpired external tokens.

OAuth protocol and SCIM bearer requests have their own authentication rules and
must route before the browser-cookie API's blanket Origin/CSRF guard. Native
administration and interaction decisions retain that guard. Enable CORS only
where required and only for explicitly registered public-client origins. Trust
TLS-forwarding headers only from the configured local reverse proxy; never enable
unrestricted proxy trust to satisfy the provider's HTTPS checks.

## SpacetimeDB persistence

Implement the provider's persistent adapter directly over private service-owned
tables. Store clients, grants, interactions, provider sessions, authorization
codes and access/refresh state with explicit expiry and indexes for grant, client,
account and originating native session. The default in-memory adapter is forbidden.
One-time consume and revocation operations must be atomic database mutations;
check the pinned library's concurrency contract with simultaneous redemption tests.

Protect secret-bearing provider payloads with the AES-GCM keyring and keep
lookup digests/indexes separate from encrypted content. Never expose provider
model tables to browser SQL, reuse a database-owner token as an access token, or
log client secrets, codes, provisioning credentials or bearer tokens. Bound and
dispose expiry/cleanup work through the existing Cordis runtime.

An additive directory profile stores SCIM attributes against existing immutable
account IDs. Native account creation and SCIM creation both use the same account
and authorization logic; no parallel shadow identities. Add numbered migration
records and a tested existing-database upgrade. Invalidate library caches when
client metadata changes; caches are never permission authority.

## SCIM 2.0 functionality

Expose `/scim/v2/Users`, `/Groups`, `/ServiceProviderConfig`, `/Schemas` and
`/ResourceTypes` with SCIM media types, schema URNs, errors, locations, metadata,
one-based pagination, supported filtering, attribute inclusion/exclusion and
revision-backed ETags. SCIM collections use SCIM ListResponse, not the browser's
`items/nextCursor` envelope. Respect the protocol's conditional-write semantics;
do not accidentally make mandatory browser If-Match rules break provisioning clients.

Support User create/read/replace/PATCH/deactivate/delete, stable IDs, userName,
externalId, displayName, structured name, active and multi-value emails; preserve
supported enterprise profile fields. Enforce case-insensitive username uniqueness,
primary-value rules and schema mutability/readOnly/writeOnly attributes. Passwords
are write-only and pass through bounded native hashing; never echo or store them
in SCIM documents. Provisioned users without a local password cannot log in with
an empty/default credential.

Support Group CRUD/PATCH, membership references, duplicate handling and explicit
unsupported nested-group behavior. Membership is represented once and projected
consistently into Users and OIDC group claims. Directory groups do not silently
grant Dar administrator rights or file/calendar permissions.

Use scoped, expiring, revocable provisioning bearer credentials administered by
an existing native administrator. Store only their digests and show secrets once.
Do not accept browser cookies, DAV passwords or ordinary OIDC profile tokens as
SCIM administrator authority. Reauthentication protects credential issuance and
other sensitive administration changes.

SCIM deactivation immediately revokes native, DAV and provider access. SCIM delete
creates a directory tombstone and disables access; it does not delete owned Files,
Contacts or Calendar content. Deleted users return SCIM 404, immutable IDs remain
reserved, and the last/native operator administrator cannot be removed or
privilege-modified by provisioning clients. Repeated deletion, rename collisions,
group cleanup and reactivation need explicit tests and documented behavior.

Implement the common filter/PATCH paths precisely and reject unsupported grammar
with SCIM errors rather than ignoring filters. Bound filters, PATCH operations,
request bodies, page sizes and group membership updates. Bulk and arbitrary
schema extensions remain disabled unless separately implemented; discovery must
accurately report those capabilities.

## Native administration and consent UI

Keep the selected Nextcloud-style Material 3 surfaces and centered search.
Provide an administrator Identity area for applications, directory users/groups,
SCIM credentials and signing-key status. Ordinary users see connected applications
and consent details, not directory administration. Secrets are revealed only at
creation/rotation and require a deliberate copy/save step.

The authorization screen names the registered application, requesting account,
destination and individual requested permissions; allow/deny and account switching
are accessible and explicit. Never render client-supplied HTML or unapproved
logos/URLs. Login has the existing native password flow and a safe bound return
to the original interaction, without an open redirect.

Static alternatives are in `identity-provider-mocks.html`. They retain the same
capabilities and differ only in administration layout and interaction presentation.
AGENTS.md requires a selection before UI implementation.

## Backup and verification

Advance the instance backup format to include directory profiles/groups, client
registrations and encrypted external signing keys. Retain tested restore of v1
and v2 backups. Restoring an instance invalidates codes, sessions, access/refresh
grants and provisioning credentials; require reauthentication/reissuance instead
of resurrecting archived bearer authority. Preserve user IDs, application data
and configured clients, with encrypted secret storage and clear recovery guidance.

| Surface | Acceptance proof | Status |
| --- | --- | --- |
| Upgrade | Existing accounts and app data survive new private tables/profile defaults | Pending |
| OIDC positive path | Independent openid-client RP: discovery, native login, consent, S256 exchange, ID token and UserInfo | Pending |
| OIDC negative paths | Concurrent code redemption, bad verifier/client/URI, expiry, denial, prompt/nonce handling, cross-site interaction rejection | Pending |
| Token lifecycle | Refresh rotation/replay, client disable/rotation, grant revoke, password change and suspension | Pending |
| Keys and logout | External key rotation with retained JWKS, internal SDK isolation, safe RP logout redirects | Pending |
| SCIM Users | Create, rename, replace/PATCH, write-only password, filters/pages/projections, deactivation, tombstone and reactivation | Pending |
| SCIM Groups | Membership add/remove/replace, invalid refs, User membership projection and OIDC group claims | Pending |
| Provisioning isolation | Dedicated credential scopes/expiry/revoke, no escalation or cross-resource token reuse | Pending |
| Browser | Admin and ordinary-user flows, real external authorization redirect, consent deny/allow, desktop/phone, focus and console | Pending |
| Persistence | Restart during flows, encrypted-at-rest payloads, backup/restore invalidation and deterministic cleanup | Pending |
| Existing apps | Retain the complete auth/Files/Contacts/Calendar acceptance suite | Pending |

## References

- [Pinned provider source and dependencies](https://github.com/panva/node-oidc-provider/blob/v9.13.3/package.json)
- [Provider integration/configuration](https://github.com/panva/node-oidc-provider/blob/v9.13.3/docs/README.md)
- [Persistent adapter contract](https://github.com/panva/node-oidc-provider/blob/v9.13.3/example/my_adapter.js)
- [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html)
- [OAuth security BCP](https://www.rfc-editor.org/rfc/rfc9700)
- [SCIM schema](https://www.rfc-editor.org/rfc/rfc7643)
- [SCIM protocol](https://www.rfc-editor.org/rfc/rfc7644)
