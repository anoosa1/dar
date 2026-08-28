import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, generateKeyPairSync, sign } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { createServer } from "node:net";
import path from "node:path";
import test from "node:test";
import { DatabaseSync } from "node:sqlite";

const repositoryRoot = path.resolve(import.meta.dirname, "..");
const serverEntry = path.join(repositoryRoot, "dist/server/main.js");

async function availablePort() {
  const probe = createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", resolve);
  });
  const { port } = probe.address();
  await new Promise((resolve, reject) => probe.close((error) => error ? reject(error) : resolve()));
  return port;
}

function startDar(dataRoot, port, extraEnvironment = {}) {
  const environment = { ...process.env };
  for (const name of [
    "DAR_BASE_URL",
    "DAR_OIDC_ISSUER",
    "DAR_OIDC_CLIENT_ID",
    "DAR_OIDC_CLIENT_SECRET",
    "DAR_OIDC_ADMIN_CLAIM",
    "DAR_OIDC_ADMIN_VALUE",
    "DAR_DEVELOPMENT",
  ]) {
    delete environment[name];
  }
  const child = spawn(process.execPath, [serverEntry], {
    cwd: repositoryRoot,
    env: {
      ...environment,
      DAR_HOST: "127.0.0.1",
      DAR_PORT: String(port),
      DAR_DATA_ROOT: dataRoot,
      ...extraEnvironment,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let output = "";
  child.stdout.on("data", (chunk) => { output += chunk; });
  child.stderr.on("data", (chunk) => { output += chunk; });
  return { child, output: () => output };
}

function sendJson(response, status, body, headers = {}) {
  response.writeHead(status, { "content-type": "application/json", ...headers });
  response.end(JSON.stringify(body));
}

async function readRequestBody(request) {
  let body = "";
  for await (const chunk of request) body += chunk;
  return body;
}

async function fakeOidcProvider() {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const publicJwk = publicKey.export({ format: "jwk" });
  Object.assign(publicJwk, { kid: "test-key", use: "sig", alg: "RS256" });
  const grants = new Map();
  let issuer;

  const server = createHttpServer(async (request, response) => {
    const url = new URL(request.url, issuer);
    if (url.pathname === "/.well-known/openid-configuration") {
      sendJson(response, 200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        jwks_uri: `${issuer}/jwks`,
        response_types_supported: ["code"],
        subject_types_supported: ["public"],
        id_token_signing_alg_values_supported: ["RS256"],
        token_endpoint_auth_methods_supported: ["client_secret_post"],
        code_challenge_methods_supported: ["S256"],
      });
      return;
    }
    if (url.pathname === "/jwks") {
      sendJson(response, 200, { keys: [publicJwk] });
      return;
    }
    if (url.pathname === "/authorize") {
      const code = `code-${grants.size + 1}`;
      grants.set(code, {
        challenge: url.searchParams.get("code_challenge"),
        nonce: url.searchParams.get("nonce"),
        redirectUri: url.searchParams.get("redirect_uri"),
      });
      const callback = new URL(url.searchParams.get("redirect_uri"));
      callback.searchParams.set("code", code);
      callback.searchParams.set("state", url.searchParams.get("state"));
      response.writeHead(302, { location: callback.href });
      response.end();
      return;
    }
    if (url.pathname === "/token" && request.method === "POST") {
      const parameters = new URLSearchParams(await readRequestBody(request));
      const grant = grants.get(parameters.get("code"));
      const challenge = createHash("sha256")
        .update(parameters.get("code_verifier"))
        .digest("base64url");
      if (!grant || challenge !== grant.challenge || parameters.get("client_secret") !== "test-secret") {
        sendJson(response, 400, { error: "invalid_grant" });
        return;
      }
      grants.delete(parameters.get("code"));
      const now = Math.floor(Date.now() / 1000);
      const header = Buffer.from(JSON.stringify({ alg: "RS256", kid: "test-key", typ: "JWT" })).toString("base64url");
      const payload = Buffer.from(JSON.stringify({
        iss: issuer,
        aud: "dar-test",
        sub: "user-123",
        nonce: grant.nonce,
        iat: now,
        exp: now + 300,
        name: "Test User",
        email: "user@example.test",
        groups: ["users", "dar-admins"],
      })).toString("base64url");
      const signingInput = `${header}.${payload}`;
      const signature = sign("RSA-SHA256", Buffer.from(signingInput), privateKey).toString("base64url");
      sendJson(response, 200, {
        access_token: "access-token",
        token_type: "Bearer",
        expires_in: 300,
        id_token: `${signingInput}.${signature}`,
      }, { "cache-control": "no-store" });
      return;
    }
    response.writeHead(404);
    response.end();
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  issuer = `http://127.0.0.1:${server.address().port}`;
  return {
    issuer,
    close: () => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())),
  };
}

async function waitForHealth(port, child, output) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`Dar exited before health was ready:\n${output()}`);
    }
    try {
      const response = await fetch(`http://127.0.0.1:${port}/health`);
      if (response.ok) return response.json();
    } catch {
      // The listener may not be bound yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`timed out waiting for Dar:\n${output()}`);
}

async function stopDar(child) {
  if (child.exitCode !== null) return child.exitCode;
  const exited = new Promise((resolve, reject) => {
    child.once("exit", resolve);
    child.once("error", reject);
  });
  child.kill("SIGTERM");
  return exited;
}

async function oversizedRequest(port) {
  return new Promise((resolve, reject) => {
    const request = httpRequest({
      host: "127.0.0.1",
      port,
      path: "/api/v1/meta",
      method: "POST",
      headers: {
        origin: `http://127.0.0.1:${port}`,
        "content-length": String(1024 * 1024 + 1),
      },
    }, (response) => {
      let body = "";
      response.setEncoding("utf8");
      response.on("data", (chunk) => { body += chunk; });
      response.on("end", () => resolve({ response, body: JSON.parse(body) }));
    });
    request.once("error", reject);
    request.end(Buffer.alloc(1024 * 1024 + 1));
  });
}

test("fresh startup creates a private versioned SQLite instance and shuts down", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-process-test-");
  const port = await availablePort();
  const running = startDar(dataRoot, port);
  try {
    assert.deepEqual(await waitForHealth(port, running.child, running.output), {
      status: "ok",
      service: "dar",
      runtime: "node",
      apiVersion: "1",
    });

    const secured = await fetch(`http://127.0.0.1:${port}/api/v1/meta`);
    assert.equal(secured.headers.get("x-content-type-options"), "nosniff");
    assert.equal(secured.headers.get("x-frame-options"), "DENY");
    assert.match(secured.headers.get("content-security-policy"), /frame-ancestors 'none'/);
    assert.match(secured.headers.get("x-request-id"), /^[0-9a-f-]{36}$/);
    assert.equal(secured.headers.get("cache-control"), "no-store");

    const rejectedOrigin = await fetch(`http://127.0.0.1:${port}/api/v1/meta`, {
      method: "POST",
      headers: { origin: "https://evil.example" },
    });
    assert.equal(rejectedOrigin.status, 403);
    const rejectedBody = await rejectedOrigin.json();
    assert.equal(rejectedBody.error.code, "forbidden");
    assert.equal(rejectedBody.error.requestId, rejectedOrigin.headers.get("x-request-id"));

    const missingSession = await fetch(`http://127.0.0.1:${port}/api/v1/session`);
    assert.equal(missingSession.status, 401);
    assert.equal((await missingSession.json()).error.code, "unauthenticated");

    const unavailableLogin = await fetch(`http://127.0.0.1:${port}/api/v1/auth/login`, {
      redirect: "manual",
    });
    assert.equal(unavailableLogin.status, 503);
    assert.equal((await unavailableLogin.json()).error.code, "unavailable");

    const sameOriginWithoutSession = await fetch(`http://127.0.0.1:${port}/api/v1/meta`, {
      method: "POST",
      headers: { origin: `http://127.0.0.1:${port}` },
    });
    assert.equal(sameOriginWithoutSession.status, 401);

    const oversized = await oversizedRequest(port);
    assert.equal(oversized.response.statusCode, 413);
    assert.equal(oversized.body.error.code, "tooLarge");

    const marker = JSON.parse(await readFile(path.join(dataRoot, "instance.json"), "utf8"));
    assert.equal(marker.product, "dar");
    assert.equal(marker.dataVersion, 1);
    assert.equal((await stat(path.join(dataRoot, "instance.json"))).mode & 0o777, 0o600);
    assert.equal((await stat(path.join(dataRoot, "dar.sqlite"))).mode & 0o777, 0o600);
    assert.equal((await stat(path.join(dataRoot, "users"))).mode & 0o777, 0o700);
    assert.equal((await stat(path.join(dataRoot, "plugins"))).mode & 0o777, 0o700);

    assert.equal(await stopDar(running.child), 0);
    assert.match(running.output(), /server closing/);

    const database = new DatabaseSync(path.join(dataRoot, "dar.sqlite"), { readOnly: true });
    assert.equal(database.prepare("SELECT version FROM dar_migrations").get().version, 1);
    database.close();
  } finally {
    await stopDar(running.child);
    await rm(dataRoot, { recursive: true, force: true });
  }
});

test("startup rejects an unmarked Go-era data root without modifying it", async () => {
  const dataRoot = await mkdtemp("/tmp/dar-legacy-root-test-");
  const legacyDatabase = path.join(dataRoot, "dar.db");
  await writeFile(legacyDatabase, "legacy");
  await mkdir(path.join(dataRoot, "legacy-files"));
  const port = await availablePort();
  const running = startDar(dataRoot, port);
  try {
    const exitCode = await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error(`Dar did not reject the root:\n${running.output()}`)), 10_000);
      running.child.once("exit", (code) => {
        clearTimeout(timeout);
        resolve(code);
      });
      running.child.once("error", reject);
    });
    assert.equal(exitCode, 1);
    assert.equal(await readFile(legacyDatabase, "utf8"), "legacy");
    await assert.rejects(stat(path.join(dataRoot, "instance.json")), { code: "ENOENT" });
  } finally {
    await stopDar(running.child);
    await rm(dataRoot, { recursive: true, force: true });
  }
});

test("OIDC PKCE login creates an administrator session and CSRF logout revokes it", async () => {
  const provider = await fakeOidcProvider();
  const dataRoot = await mkdtemp("/tmp/dar-oidc-process-test-");
  const port = await availablePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const running = startDar(dataRoot, port, {
    DAR_BASE_URL: baseUrl,
    DAR_OIDC_ISSUER: provider.issuer,
    DAR_OIDC_CLIENT_ID: "dar-test",
    DAR_OIDC_CLIENT_SECRET: "test-secret",
    DAR_DEVELOPMENT: "true",
    DAR_OIDC_ADMIN_CLAIM: "groups",
    DAR_OIDC_ADMIN_VALUE: "dar-admins",
  });
  try {
    await waitForHealth(port, running.child, running.output);

    const login = await fetch(`${baseUrl}/api/v1/auth/login`, { redirect: "manual" });
    assert.equal(login.status, 303, running.output());
    const flowCookie = login.headers.getSetCookie()
      .find((value) => value.startsWith("dar-oidc-flow="))
      .split(";", 1)[0];

    const authorization = await fetch(login.headers.get("location"), { redirect: "manual" });
    assert.equal(authorization.status, 302);

    const callback = await fetch(authorization.headers.get("location"), {
      redirect: "manual",
      headers: { cookie: flowCookie },
    });
    assert.equal(callback.status, 303, running.output());
    assert.equal(callback.headers.get("location"), "/");
    const setCookies = callback.headers.getSetCookie();
    assert.ok(setCookies.some((value) => value.startsWith("dar-session=")), JSON.stringify(setCookies));
    assert.ok(setCookies.some((value) => value.startsWith("dar-csrf=")), JSON.stringify(setCookies));
    const sessionCookie = setCookies
      .find((value) => value.startsWith("dar-session="))
      .split(";", 1)[0];
    const csrfCookie = setCookies
      .find((value) => value.startsWith("dar-csrf="))
      .split(";", 1)[0];
    const csrfToken = csrfCookie.slice("dar-csrf=".length);
    const cookies = `${sessionCookie}; ${csrfCookie}`;

    const session = await fetch(`${baseUrl}/api/v1/session`, {
      headers: { cookie: cookies },
    });
    assert.equal(session.status, 200);
    const sessionBody = await session.json();
    assert.equal(sessionBody.user.subject, "user-123");
    assert.equal(sessionBody.user.displayName, "Test User");
    assert.equal(sessionBody.user.administrator, true);

    const authHeaders = {
      cookie: cookies,
      origin: baseUrl,
      "content-type": "application/json",
      "x-csrf-token": csrfToken,
    };
    const createdPreference = await fetch(`${baseUrl}/api/v1/preferences/calendar/view`, {
      method: "PUT",
      headers: authHeaders,
      body: JSON.stringify({ layout: "week" }),
    });
    assert.equal(createdPreference.status, 200);
    const createdBody = await createdPreference.json();
    assert.equal(createdBody.key, "view");
    assert.deepEqual(createdBody.value, { layout: "week" });
    assert.equal(createdBody.revision, 1);
    const firstEtag = createdPreference.headers.get("etag");
    assert.equal(firstEtag, '"1"');

    const missingIfMatch = await fetch(`${baseUrl}/api/v1/preferences/calendar/view`, {
      method: "PUT",
      headers: authHeaders,
      body: JSON.stringify({ layout: "month" }),
    });
    assert.equal(missingIfMatch.status, 409);
    assert.equal((await missingIfMatch.json()).error.code, "conflict");

    const updatedPreference = await fetch(`${baseUrl}/api/v1/preferences/calendar/view`, {
      method: "PUT",
      headers: { ...authHeaders, "if-match": firstEtag },
      body: JSON.stringify({ layout: "month" }),
    });
    assert.equal(updatedPreference.status, 200);
    const updatedBody = await updatedPreference.json();
    assert.equal(updatedBody.revision, 2);
    assert.equal(updatedBody.value.layout, "month");

    const preferenceList = await fetch(`${baseUrl}/api/v1/preferences/calendar`, {
      headers: { cookie: cookies },
    });
    assert.equal(preferenceList.status, 200);
    assert.deepEqual(await preferenceList.json(), {
      items: [{
        key: "view",
        value: { layout: "month" },
        revision: 2,
        updatedAt: updatedBody.updatedAt,
      }],
    });

    const deletedPreference = await fetch(`${baseUrl}/api/v1/preferences/calendar/view`, {
      method: "DELETE",
      headers: { ...authHeaders, "if-match": updatedPreference.headers.get("etag") },
    });
    assert.equal(deletedPreference.status, 200);

    const logout = await fetch(`${baseUrl}/api/v1/auth/logout`, {
      method: "POST",
      headers: {
        cookie: cookies,
        origin: baseUrl,
        "x-csrf-token": csrfToken,
      },
    });
    assert.equal(logout.status, 200);
    assert.deepEqual(await logout.json(), { ok: true });

    const revoked = await fetch(`${baseUrl}/api/v1/session`, {
      headers: { cookie: cookies },
    });
    assert.equal(revoked.status, 401);
  } finally {
    await stopDar(running.child);
    await provider.close();
    await rm(dataRoot, { recursive: true, force: true });
  }
});
