import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import * as oidc from "openid-client";
import { database } from "../runtime/database.ts";
import { providerStore } from "../runtime/identity/store.ts";
import { configureOidc } from "../runtime/identity/config.ts";
import { identityAdmin, authorize, exchange } from "./identity-helpers.ts";

test("only explicitly opted-out confidential clients may omit PKCE and still require their secret", { skip: !process.env.DAR_DATABASE_URL }, async () => {
  const base = process.env.DAR_INTEGRATION_ORIGIN!;
  const db = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  const store = await providerStore(db, path.join(process.env.DAR_TEST_ROOT!, "secrets"));
  const admin = await identityAdmin(base), directory = await mkdtemp(path.join(tmpdir(), "dar-pkce-"));
  try {
    const secret = "isolated-pkce-compatibility-secret-1234567890", id = "gitea-compatible-rp", redirect = "http://127.0.0.1:49999/callback";
    await writeFile(path.join(directory, "secret"), secret, { mode: 0o600 });
    const entry = { id, name: "Gitea compatible", type: "confidential", redirectUris: [redirect], scopes: ["openid", "profile"], secretFile: "secret" };
    const filename = path.join(directory, "oidc.json");
    async function apply(value: unknown) { await writeFile(filename, JSON.stringify({ clients: [value] })); await configureOidc(filename, store, true); }
    const config = await oidc.discovery(new URL(`${base}/oidc`), id, secret, oidc.ClientSecretBasic(secret), { execute: [oidc.allowInsecureRequests] });
    const client = { config, redirect };
    const flow = () => authorize(base, admin.session, client, { pkce: false, scope: "openid profile" });
    await apply(entry);
    assert.equal((await flow()).url.searchParams.get("error"), "invalid_request", "default requires PKCE");
    await assert.rejects(apply({ ...entry, requirePkce: "false" }));
    const { secretFile: _file, ...publicEntry } = entry;
    await assert.rejects(apply({ ...publicEntry, type: "public", requirePkce: false }));
    await apply({ ...entry, requirePkce: false });
    const detail = await admin.api(`/clients/${id}`);
    assert.equal((await detail.json() as { requirePkce: boolean }).requirePkce, false);
    const allowed = await flow();
    const tokens = await oidc.authorizationCodeGrant(config, allowed.url, { expectedState: allowed.state, expectedNonce: allowed.nonce });
    assert.ok(tokens.id_token);
    const wrongSecretFlow = await flow();
    const rejected = await fetch(`${base}/oidc/token`, { method: "POST", headers: { authorization: `Basic ${Buffer.from(`${id}:wrong-secret`).toString("base64")}`, "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams({ grant_type: "authorization_code", redirect_uri: redirect, code: wrongSecretFlow.url.searchParams.get("code")! }) });
    assert.equal(rejected.status, 401);
    const protectedFlow = await authorize(base, admin.session, client, { scope: "openid profile" });
    await assert.rejects(oidc.authorizationCodeGrant(config, protectedFlow.url, { pkceCodeVerifier: oidc.randomPKCECodeVerifier(), expectedState: protectedFlow.state, expectedNonce: protectedFlow.nonce }), "supplied PKCE remains enforced");
    await apply(entry);
    assert.equal((await flow()).url.searchParams.get("error"), "invalid_request", "re-enabling PKCE applies to the next request");
    const publicClient = await admin.client("public", "openid");
    assert.equal((await authorize(base, admin.session, publicClient, { pkce: false, scope: "openid" })).url.searchParams.get("error"), "invalid_request");
    await apply({ ...entry, requirePkce: false }); // Leave the configured client for the browser edit/persistence check.
  } finally { await rm(directory, { recursive: true, force: true }); }
});

test("text-configured OIDC clients persist, authenticate, rotate and disable with revision checks", { skip: !process.env.DAR_DATABASE_URL }, async () => {
  const base = process.env.DAR_INTEGRATION_ORIGIN!;
  const db = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  const store = await providerStore(db, path.join(process.env.DAR_TEST_ROOT!, "secrets"));
  const id = "configured-rp", secret = "isolated-configured-secret-1234567890";
  const read = () => store.call<{ revision: number; metadata: string; encrypted: string; enabled: boolean }>("client", { id });
  const original = await read();
  assert.equal(original.revision, 1);
  assert.equal(original.metadata.includes(secret), false);
  assert.equal(original.encrypted.includes(secret), false);
  const client = { config: await oidc.discovery(new URL(`${base}/oidc`), id, secret, oidc.ClientSecretBasic(secret), { execute: [oidc.allowInsecureRequests] }), redirect: "http://127.0.0.1:49999/callback" };
  const admin = await identityAdmin(base);
  const tokens = await exchange(client, await authorize(base, admin.session, client, { scope: "openid profile" }));
  assert.ok(tokens.access_token);
  const directory = await mkdtemp(path.join(tmpdir(), "dar-oidc-config-"));
  try {
    const filename = path.join(directory, "oidc.json");
    const entry = { id, name: "Configured RP", type: "confidential", redirectUris: [client.redirect], scopes: ["openid", "profile"], secretFile: "secret" };
    await writeFile(path.join(directory, "secret"), `${secret}\n`, { mode: 0o600 });
    const apply = async (clients: unknown[]) => { await writeFile(filename, JSON.stringify({ clients })); await configureOidc(filename, store, true); };
    await apply([entry]);
    assert.equal((await read()).revision, 1, "unchanged settings retain grants and revision");
    await apply([]);
    assert.equal((await read()).revision, 1, "omitted clients are retained");
    const publicId = "configured-public-rp";
    await apply([{ id: publicId, name: "Public RP", type: "public", redirectUris: ["https://public.example.test/callback"], scopes: ["openid"] }]);
    assert.equal((await new store.Adapter("Client").find(publicId))?.token_endpoint_auth_method, "none");
    await assert.rejects(apply([{ ...entry, redirectUris: ["https://photos.example.test/*"] }]));
    await assert.rejects(apply([{ ...entry, name: "Should not apply" }, { ...entry }]));
    assert.equal((await read()).revision, 1, "validate the entire file before writing");
    await writeFile(path.join(directory, "secret"), "rotated-isolated-secret-1234567890");
    await apply([entry]);
    assert.equal((await read()).revision, 2);
    await assert.rejects(store.call("saveClient", { id, revision: 1, metadata: "{}", encrypted: "stale", enabled: true }));
    await assert.rejects(oidc.fetchUserInfo(client.config, tokens.access_token, oidc.skipSubjectCheck));
    await apply([{ ...entry, enabled: false }]);
    assert.equal((await read()).enabled, false);
    const adapter = new store.Adapter("Client");
    assert.equal(await adapter.find(id), undefined);
    await assert.rejects(configureOidc(path.join(directory, "missing"), store, true));
    await configureOidc(path.join(directory, "missing"), store, true, true);
    await writeFile(filename, '{"clients":["sensitive-invalid-content"]');
    await assert.rejects(configureOidc(filename, store, true), { message: "Invalid OIDC configuration JSON." });
  } finally { await rm(directory, { recursive: true, force: true }); }
});
