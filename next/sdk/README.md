# Dar app SDK v1

Installed apps are trusted administrator-approved code. Node hooks and same-origin
browser code are not sandboxed. Never put secrets in a browser bundle or manifest.
The SDK is experimental v1; manifests use `apiVersion: 1` and exact package versions.

## Host integration

- `AppWorkspace` renders your sidebar, content and optional details inside Dar's
  shared frame (app rail, top bar, sheets). The top-bar search always covers all
  apps; `query`, `onQuery` and `searchLabel` are accepted for older builds but do
  not scope it. Enabled apps' own-user records are searched with
  `ownUserData:read`, without showing record contents.
- Pass `onSettings` to show the app's settings gear in the top bar, and keep a
  sidebar `nav-item ui-app-settings-entry` button too: phones show it as the last
  chip. Settings use the shared `SettingsSheet` (tabs, Saved status and Undo)
  with `SettingRow`, `SettingSwitch` and `Segmented` rows; save each valid
  change and never save invalid text.

## Build an app outside Dar

Maintainers can distribute the SDK without publishing to npm:

```sh
cd next/sdk
npm pack --pack-destination /tmp
```

In your own project, install the resulting tarball as a development dependency:

```sh
npm init -y
npm install --save-dev /path/to/dar-app-sdk-0.1.0.tgz
```

Create `app.json`:

```json
{
  "id": "org.example.clock",
  "name": "Clock",
  "version": "0.1.0",
  "apiVersion": 1,
  "license": "MIT",
  "client": "src/client.tsx",
  "permissions": ["ownUserData:read", "ownUserData:write"]
}
```

Create `src/client.tsx`:

```tsx
/** @jsxImportSource @dar/app-sdk */
import {createRoot, AppWorkspace, useState, type AppHost} from '@dar/app-sdk';

function Clock({host}: {host: AppHost}) {
  const [query, setQuery] = useState('');
  return <AppWorkspace host={host} query={query} onQuery={setQuery}
    searchLabel="Search clock" sidebar={<p>Clock</p>}>
    <h1>My clock</h1>
  </AppWorkspace>;
}

export function mount(element: HTMLElement, host: AppHost) {
  const root = createRoot(element, {onUncaughtError: host.onError});
  root.render(<Clock host={host}/>);
  return () => root.unmount();
}
```

Use `npx dar-app-build .`. It bundles dependencies, externalizes the host SDK,
extracts imported CSS and writes a complete `dist/dar-app.json` file inventory.
Only `dist/` is installed. No package installation or compilation happens on the
production host. Import React hooks and the root from this SDK, not a bundled
second React. CSS must be scoped to your app's own class names; use Dar theme
variables. There are no runtime CDN imports, inline styles or browser prompts.

For TypeScript checking use `jsx: react-jsx`, `jsxImportSource: @dar/app-sdk`,
`moduleResolution: Bundler`, DOM libraries and the SDK's declarations. The
versioned `/sdk.js` runtime is provided by the host, not an npm runtime export.

## User data

`host.data.get<T>(key)` returns a record or null. `put(key, value, revision,
schemaVersion = 1)` creates only at revision 0, otherwise requires the exact
previous revision. `delete(key, revision)` is conditional. `list(cursor?)` returns
up to 50 records with a next cursor. Keys use letters, numbers, dot, dash and
underscore, beginning with a letter or number; maximum 100 characters.

Always validate saved payloads at runtime, preserve edits on conflict, and make
migrations restartable. Generic types do not validate JSON. Limits: 64 KiB per
record, 1 MiB and 128 records per account/app. No arbitrary SQL or cross-app data
access is exposed. A fetch in progress aborts when the app is disposed. Do not
log user settings. App data is retained when the operator removes the package.

Declare `ownUserData:read` and/or `ownUserData:write` as needed. Declare
`regional:read` to call `host.regional()` for the account's default timezone.
Do not use browser credentials or the visible session to bypass supported SDK
operations; code is trusted, so this is an author obligation, not a sandbox claim.

## Optional Node hook

Set `server` in `app.json` to your Node entry. Export `activate(context)` returning
`{handle(request)}`. Requests under `/api/v1/apps/{appId}/api/...` pass native
session and CSRF checks first. The request includes method, relative path, query,
bounded JSON body, authenticated user, abort signal and scoped data helpers.
Return a JSON value. No raw database token, response object or arbitrary router is
provided. Use the same declared own-user data permissions.

Use `context.effect(() => { const timer = setInterval(...); return () =>
clearInterval(timer); })` and `context.signal`. Effects are owned by a per-app
Cordis context. Startup failures mark that app unavailable; recovery mode can
disable all external apps. Code that blocks Node's event loop or exits the process
cannot be contained by this trusted in-process API.

See the separately packaged `apps/prayer-times` example and `docs/apps.md` in Dar.
