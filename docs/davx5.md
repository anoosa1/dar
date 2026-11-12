# DAV sync (DAVx⁵)

## Contacts and Calendar

1. Serve Dar over valid HTTPS reachable from the phone.
2. Create the address books and calendars in Dar.
3. In **Settings → Security → App passwords**, create a password with Contacts
   and Calendar Read and Write (Read only for one-way sync).
4. In DAVx⁵, choose **Login with URL and user name**: `https://cloud.example.com/dav/`,
   your Dar username and the app password. Never use your sign-in password.
5. Select the discovered collections and sync. If apps show nothing, check
   Android's account sync settings and permissions.

`/.well-known/carddav`, `/.well-known/caldav` and `/dav` redirect to `/dav/`.
Supported: principal and home discovery, PROPFIND, ETags/ctags, query and
multiget REPORTs, conditional create/update/delete, vCard 3/4, VEVENT and VTODO,
using the same authorization and storage as the browser. Not advertised:
sync-collection, LOCK, PROPPATCH, collection creation over DAV and scheduling;
clients fall back to ETag/ctag queries. Revoking the password, suspending the
account or recovering its password ends DAV access. Older `pim:read`/`pim:write`
passwords keep Contacts/Calendar access only.

Server tests and vdirsyncer round trips cover these paths; this is not an Android
certification. If discovery fails, check the URL and TLS from the phone (its
`localhost` is the phone), the username, the password's grants and that the
proxy passes DAV methods and well-known redirects. Never share credentials or
authorization headers in bug reports.

## Files

Files mount separately through DAVx⁵ **Tools → WebDAV mounts**. Create a password
with Files Read (and Write to edit), then mount
`https://cloud.example.com/dav/files/<username>/`. Dar shows this address when it
issues a Files password, and `/dav/` lists a Files home only for credentials with
Files Read. Responses advertise `DAV: 1`, including OPTIONS. Browse the mount
through Android's system file picker. See the
[WebDAV mounts guide](https://manual.davx5.com/webdav_mounts.html) and
[account guide](https://manual.davx5.com/accounts_collections.html).
