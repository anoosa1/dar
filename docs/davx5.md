# DAVx⁵ connection

1. Deploy Dar behind valid HTTPS reachable from the Android device.
2. In Dar, create the address books and calendars you want to synchronize.
3. Open **Settings → Personal → Security → App passwords**. Create a credential
   for **DAVx⁵ · Contacts and Calendar · Read and write** (or read only).
4. In DAVx⁵, add an account using **Login with URL and user name**. Use
   `https://cloud.example.com/dav/`, the Dar username, and the generated app password.
5. Select the discovered address books/calendars in DAVx⁵ and run synchronization.
   Check Android's permissions and account sync settings if local apps show no data.

The combined credential uses `pim:read` or `pim:write`. It grants Contacts/Calendar
protocol access only; it cannot access Files, native browser APIs, OIDC admin or
SCIM. Existing per-app credentials stay narrowly scoped. You may instead create
separate DAVx⁵ accounts using Contacts and Calendar passwords and their displayed
home URLs. Do not use your ordinary sign-in password for DAV.

The server redirects `/.well-known/carddav`, `/.well-known/caldav` and `/dav` to
`/dav/`. Discovery returns the current principal and both home sets. Collection
PROPFIND, ETags, ctags, query/multiget REPORTs, conditional creation/update/delete,
vCard 3/4 and VEVENT/VTODO use the same authorization and storage as the native UI.
Password revocation, account suspension and password recovery revoke protocol
access. Credentials are shown once; recreate one if you lose it.

Sync tokens/sync-collection, LOCK, PROPPATCH, collection creation through DAV and
invitation scheduling are not advertised. Clients use ETag/ctag/query fallback.
DAVx⁵-side testing on Android is still required; the automated evidence is server
discovery/conditional-write tests and independent vdirsyncer round trips, not an
Android certification claim.

If discovery fails, verify the URL is reachable from the phone, TLS is valid, the
username is correct, the app password has Contacts/Calendar access, and the proxy
preserves the DAV methods and well-known redirects. A phone's `localhost` is the
phone itself; use the deployed hostname. Never send credentials or authorization
headers in a bug report.

[DAVx⁵ account documentation](https://manual.davx5.com/accounts_collections.html)
