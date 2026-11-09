# Native OIDC and SCIM provider

Requested November 6, 2026, after native auth, Files, Contacts and Calendar.
Branch: `codex/spacetimedb-auth-files`. The maintainer approved the pinned provider
dependencies, internal Koa, and layout B (Account settings tabs). Implemented and
verified on November 6, 2026; the acceptance evidence and supported subset follow.

## Provider boundary

Dar remains the identity provider and owns passwords, login and its directory.
Other applications use Dar for OIDC sign-in; provisioning clients manage Dar's
users/groups through SCIM. This does not add an external identity provider,
outbound SCIM connectors or an alternative account database.

Use `oidc-provider@9.13.3`, MIT-licensed, with `@types/oidc-provider@9.13.1`.
Its provider callback mounts on the existing Node HTTP server inside the Cordis
lifecycle. The package uses Koa internally; the maintainer approved that
transitive dependency boundary exception. Do not start another listener,
runtime, service or database. Native React interactions replace the package's
development UI. `openid-client`, already approved in the parent dependency list,
provides independent relying-party integration tests.

The maintainer explicitly approved this dependency under AGENTS.md.
Using a certified library does not mean this Dar configuration is certified.

## OIDC functionality

- Publish a dedicated external issuer at `/oidc` with discovery, JWKS,
  authorization, token, UserInfo, revocation, introspection and RP-initiated logout.
  Advertise only features actually configured and tested.
- Administer registered public and confidential clients; exact redirect and
  post-logout URI allowlists, client authentication, allowed scopes, enable/disable,
  secret rotation and deletion. No open registration endpoint or wildcard URIs.
  Production clients use HTTPS; development exceptions are explicit loopback URIs.
  Host operators can also configure clients through a JSON file or
  `services.dar.oidc.clients`; see [deployment](../deployment.md#oidc-clients).
  Startup uses the same encrypted storage, revision checks and grant revocation
  as administration. Omitted clients are retained; explicit disabling revokes access.
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

Static alternatives are in `identity-provider-mocks.html`. Layout B, Account
settings tabs, was selected before implementation as required by AGENTS.md.

## Backup and verification

Advance the instance backup format to include directory profiles/groups, client
registrations and encrypted external signing keys. Retain tested restore of v1
and v2 backups. Restoring an instance invalidates codes, sessions, access/refresh
grants and provisioning credentials; require reauthentication/reissuance instead
of resurrecting archived bearer authority. Preserve user IDs, application data
and configured clients, with encrypted secret storage and clear recovery guidance.

## Delivered protocol contract

The approved layout is at `/settings` (also `/identity`). Account security links
to it from Files, Contacts and Calendar. Registered client metadata in the browser
API uses camelCase; the encrypted provider adapter translates to standard OIDC
metadata. Directory resources carry standard SCIM names and schema URNs.

| Endpoint | Purpose |
| --- | --- |
| `/oidc/.well-known/openid-configuration`, `/oidc/jwks` | External discovery and RS256 public keys |
| `/oidc/auth`, `/oidc/token`, `/oidc/me` | Code/S256 authorization, exchange/refresh and UserInfo |
| `/oidc/token/revocation`, `/oidc/token/introspection` | Client-authenticated token lifecycle |
| `/oidc/session/end` | RP-initiated logout, with registered destinations only |
| `/scim/v2/Users`, `/scim/v2/Groups` | Provisioned native accounts and direct user memberships |
| `/scim/v2/ServiceProviderConfig`, `/Schemas`, `/ResourceTypes` | Authenticated capability/schema discovery under the SCIM base |
| `/api/v1/identity/clients`, `/users`, `/groups`, `/credentials`, `/status` | Native administration under `/api/v1/identity` |
| `/api/v1/identity/confirmation`, `/keys/rotate` | Recent password proof and external signing-key rotation |
| `/api/v1/identity/connections` | Own applications and revocation across own grants |

Clients are confidential web clients using `client_secret_basic`, or public web
clients using `none`. Both require S256 PKCE and exact registered redirects.
Production redirects use HTTPS; explicit loopback HTTP redirects are accepted in
development. Custom URI schemes, dynamic registration, implicit/hybrid/password
grants, DPoP, PAR, JWT request objects and arbitrary claim selection are disabled.
Query, fragment and form-post code responses use the library's protocol handling;
form-post rendering uses self-hosted assets. No outbound client-metadata/JWKS
fetch is registered. Access tokens are opaque and expire in five minutes, as do
RS256 ID tokens. Codes last 60 seconds and interactions ten minutes. Grants,
provider sessions and rotated refresh tokens last at most seven days, and always
remain bound to the originating active native session.

Interaction cookies remain scoped to each request. The verified interaction page
sets an additional HttpOnly copy scoped to its native API path, so separate sign-in
tabs do not overwrite one another's state. Native cookies stay Strict; provider
cookies are Lax and Secure in production. The runtime waits for the provider's
middleware and adapter writes even after an HTTP client disconnects.

Supported User profile fields are `userName`, `externalId`, `displayName`, `name`,
`active`, `emails`, write-only `password`, `title`, `userType`, `locale`, `timezone`
and `preferredLanguage`. The enterprise extension supports employee number,
cost center, organization, division, department and manager references. Unknown
attributes/schema extensions fail rather than being stored accidentally.
Name/email/enterprise attribute names are case-insensitive; identifiers remain
opaque. Native usernames retain the existing 3–64 character ASCII login format:
letters/digits followed by letters, digits, dots, underscores or hyphens. Email
addresses belong in `emails`, not an email-shaped `userName`. Renaming an ordinary
user preserves the subject/resource ID but changes their native login and DAV
home URL; DAV clients need the new username/address. Admin profiles may be updated,
but provisioning cannot rename, disable,
delete, reset the password of, or downgrade the protected operator. Native operator
password changes use Account security and the existing recovery command.

Filters support `eq`, `ne`, `co`, `sw`, `ew`, `pr`, `gt`, `ge`, `lt`, `le`, boolean
`and`/`or`/`not`, parentheses, dotted/enterprise paths and multivalued value paths.
PATCH supports add/replace/remove, including filtered emails and memberships;
choosing a new primary email clears the previous primary. Attribute projection
supports nested and multivalued fields. PUT replaces optional profile data while
preserving an omitted password. `active:false` can be reversed; tombstones cannot
be reactivated, including through the enable-account operator command. Neither
group membership nor enterprise fields map to administrator/app permissions.

SCIM `If-Match` is optional. When present it must match the revision-backed weak
ETag; stale writes fail with 412. Native editing requires `If-Match`. Conditional
database mutations still check the captured revision when a SCIM client omits the
header. Provisioning bearer scope is checked before expensive password hashing and
again in the write transaction. Operator authentication, origin and CSRF guards
remain on native decisions and administration.

Explicit bounds: 10,000 accounts including reserved tombstones, 2,000 groups,
2,000 members/group, 200 clients and 50,000 live/retained provider records. SCIM
pages contain at most 200 resources, bodies at most 256 KiB, User documents 16 KiB,
filters 2,048 characters/12 nesting levels, and PATCH at most 100 operations.
Up to 1,000 provisioning credential records are retained; revoked/expired records
can be reclaimed when issuing another credential. Proofs expire after five minutes
and are capped at 2,000 records. Existing runtime maintenance prunes expired proofs
and up to 1,000 expired provider records/minute; expired records cannot authorize
access while awaiting cleanup. OIDC routes allow 400 requests/minute per socket
peer, and password-confirmation/interaction failures have a separate 20/minute
budget. Behind the local proxy that peer budget covers the whole instance.
These are safety bounds, not a tested directory/archive capacity guarantee.

## Acceptance evidence

The complete verification command is `npm --prefix next run verify`. A focused
OIDC/SCIM pass is `node next/scripts/verify.mjs --identity` inside the Nix shell.
The suite includes independent `openid-client` exchanges, a cross-site relying
party in Chromium, parallel consent tabs, operator/profile protection, protocol
errors, private-table denial, encrypted keyring tamper/retirement, v1/v2 upgrade
and restore, v3 authority invalidation, and forced process/database restart.

The production fixture tests canonical HTTPS issuer/proxy headers, Secure cookies
and a full code exchange over a local proxy boundary. It does not install a public
TLS certificate or certify a specific reverse proxy deployment. The documented
Caddy configuration must still be adapted to the operator's host/database names.

Verification passed on November 6, 2026: the complete acceptance run contained
42 Node tests, all three desktop/phone browser journeys, vdirsyncer, complete
backup/restore and forced runtime/database restart. The final focused pass also
passed all 11 Node tests plus the identity browser journey after the API revision
and logout-cancellation checks. There are no skipped integration tests in these
runs. Local isolated artifacts for the full and final focused runs were
`/tmp/nix-shell.EkmSZh/dar-acceptance-S7QZI4` and
`/tmp/nix-shell.pj5kCU/dar-acceptance-SPGjHS`; none are committed or live user data.

| Surface | Acceptance proof | Status |
| --- | --- | --- |
| Upgrade | Existing accounts and app data survive new private tables/profile defaults | Passed |
| OIDC positive path | Independent openid-client RP: discovery, native login, consent, S256 exchange, ID token and UserInfo | Passed |
| OIDC negative paths | Concurrent code redemption, bad verifier/client/URI, expiry, denial, prompt/nonce handling, cross-site interaction rejection | Passed |
| Token lifecycle | Refresh rotation/replay, client disable/rotation, grant revoke, password change and suspension | Passed |
| Keys and logout | External key rotation with retained JWKS, internal SDK isolation, safe RP logout redirects | Passed |
| SCIM Users | Create, rename, replace/PATCH, write-only password, filters/pages/projections, deactivation, tombstone and reactivation | Passed |
| SCIM Groups | Membership add/remove/replace, invalid refs, User membership projection and OIDC group claims | Passed |
| Provisioning isolation | Dedicated credential scopes/expiry/revoke, no escalation or cross-resource token reuse | Passed |
| Browser | Admin and ordinary-user flows, real external authorization redirect, consent deny/allow, desktop/phone, focus and console | Passed |
| Persistence | Restart during flows, encrypted-at-rest payloads, backup/restore invalidation and deterministic cleanup | Passed |
| Existing apps | Retain the complete auth/Files/Contacts/Calendar acceptance suite | Passed |

## References

- [Pinned provider source and dependencies](https://github.com/panva/node-oidc-provider/blob/v9.13.3/package.json)
- [Provider integration/configuration](https://github.com/panva/node-oidc-provider/blob/v9.13.3/docs/README.md)
- [Persistent adapter contract](https://github.com/panva/node-oidc-provider/blob/v9.13.3/example/my_adapter.js)
- [OIDC Core](https://openid.net/specs/openid-connect-core-1_0.html)
- [OAuth security BCP](https://www.rfc-editor.org/rfc/rfc9700)
- [SCIM schema](https://www.rfc-editor.org/rfc/rfc7643)
- [SCIM protocol](https://www.rfc-editor.org/rfc/rfc7644)
