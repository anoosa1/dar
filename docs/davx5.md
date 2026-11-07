# DAVx⁵ connection

1. Deploy Dar behind valid HTTPS reachable from the Android device.
2. In Dar, create the address books and calendars you want to synchronize.
3. Open **Settings → Personal → Security → App passwords**. Create a credential
   with Contacts and Calendar Read/Write checked (or Read for read-only sync).
4. In DAVx⁵, add an account using **Login with URL and user name**. Use
   `https://cloud.example.com/dav/`, the Dar username, and the generated app password.
5. Select the discovered address books/calendars in DAVx⁵ and run synchronization.
   Check Android's permissions and account sync settings if local apps show no data.

The permission matrix supports multiple apps with independent Read and Write
grants. Older combined credentials (`pim:read` or `pim:write`) continue to grant
Contacts/Calendar protocol access only; they cannot access Files. No app password
grants native browser API, OIDC administration or SCIM access. You may also create
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

## Files mounts

Files use DAVx⁵ **Tools → WebDAV mounts**, separately from address-book/calendar
synchronization. Create an app password with **Files → Read** checked, plus
**Write** if you want to change files. Mount
`https://cloud.example.com/dav/files/<username>/` using your Dar username and that
password. Dar shows this Files address when issuing a Files-enabled password.
The generic `/dav/` root also lists a Files home for credentials with Files Read.
Credentials without Files access must not see that home.

Files responses advertise `DAV: 1`, including OPTIONS, so mount clients can
recognize the service. Use the full HTTPS URL and your actual username. If an
older deployment reports no DAV service even with the explicit Files address,
update the server; changing the Android path cannot add the missing DAV header.

Browse the mount in Android's system file picker or a file manager that supports
the Storage Access Framework. Merely adding a Contacts/Calendar account does not
mount Files. See the [official WebDAV mounts guide](https://manual.davx5.com/webdav_mounts.html).
Actual Android-device verification remains separate from the server tests.

[DAVx⁵ account documentation](https://manual.davx5.com/accounts_collections.html)
