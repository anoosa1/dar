// Shared process-test harness: spawns the compiled Dar server against
// isolated temporary data roots and provides a hermetic OIDC provider for
// session-based tests. Never points at a live data root.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, generateKeyPairSync, randomUUID, sign } from "node:crypto";
import { createServer as createHttpServer } from "node:http";
import { createServer as createHttpsServer } from "node:https";
import { createServer } from "node:net";
import path from "node:path";

const repositoryRoot = path.resolve(import.meta.dirname, "..", "..");
const serverEntry = path.join(repositoryRoot, "dist/server/main.js");

export async function availablePort() {
  const probe = createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", resolve);
  });
  const { port } = probe.address();
  await new Promise((resolve) => probe.close(resolve));
  return port;
}

export function startDar(dataRoot, port, extraEnvironment = {}) {
  const developmentSnapshot = process.env.DAR_SERVER_BUILD_SNAPSHOT === "development";
  if (developmentSnapshot) assert.ok(process.env.NODE_V8_COVERAGE, "The extra server snapshot is only for isolated coverage verification");
  const entry = developmentSnapshot ? path.join(repositoryRoot, "dist/server/development/main.js") : serverEntry;
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
  const child = spawn(process.execPath, [entry], {
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

export async function fakeOidcProvider(tls) {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  const publicJwk = publicKey.export({ format: "jwk" });
  Object.assign(publicJwk, { kid: "test-key", use: "sig", alg: "RS256" });
  const grants = new Map();
  const defaultIdentity = { sub: "user-123", name: "Test User", email: "user@example.test", groups: ["users", "dar-admins"] };
  let identity = defaultIdentity;
  let issuer;

  const serve = async (request, response) => {
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
      const code = `code-${randomUUID()}`;
      grants.set(code, {
        identity,
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
        ...grant.identity,
        nonce: grant.nonce,
        iat: now,
        exp: now + 300,
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
  };
  const server = tls ? createHttpsServer(tls, serve) : createHttpServer(serve);

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  issuer = `${tls ? "https" : "http"}://127.0.0.1:${server.address().port}`;
  return {
    issuer,
    setIdentity: value => { identity = { ...defaultIdentity, ...value }; },
    close: () => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())),
  };
}

export async function waitForHealth(port, child, output) {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`Dar exited before health was ready:\n${output()}`);
    }
    // /health is registered before every app has finished mounting. Wait for
    // this child's completed startup, rather than a premature or unrelated
    // listener on a concurrently allocated test port.
    if (!output().includes('Dar is ready at ')) {
      await new Promise(resolve => setTimeout(resolve, 25));
      continue;
    }
    try {
      const response = await fetch(`http://127.0.0.1:${port}/health`);
      if (response.ok) return response.json();
    } catch (error) {
      if (error.cause?.message === "bad port") {
        throw new Error(`Port ${port} is restricted by HTTP clients; choose a different preview port.`);
      }
      // The listener may not be bound yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`timed out waiting for Dar:\n${output()}`);
}

export async function stopDar(child) {
  if (child.exitCode !== null || child.signalCode !== null) return child.exitCode;
  const exited = new Promise((resolve, reject) => {
    child.once("exit", resolve);
    child.once("error", reject);
  });
  child.kill("SIGTERM");
  return exited;
}

export function cookieValue(setCookieHeader, name) {
  const raw = setCookieHeader
    .find((value) => value.startsWith(`${name}=`))
    .split(";", 1)[0];
  return { raw, value: raw.slice(`${name}=`.length) };
}

// Performs the full OIDC PKCE dance against a running Dar instance and
// returns the authenticated browser credentials.
export async function loginSession(baseUrl, output = () => "") {
  const login = await fetch(`${baseUrl}/api/v1/auth/login`, { redirect: "manual" });
  assert.equal(login.status, 303, output());
  const flowCookie = cookieValue(login.headers.getSetCookie(), "dar-oidc-flow").raw;

  const authorization = await fetch(login.headers.get("location"), { redirect: "manual" });
  assert.equal(authorization.status, 302);

  const callback = await fetch(authorization.headers.get("location"), {
    redirect: "manual",
    headers: { cookie: flowCookie },
  });
  assert.equal(callback.status, 303, output());
  const setCookies = callback.headers.getSetCookie();
  const session = cookieValue(setCookies, "dar-session");
  const csrf = cookieValue(setCookies, "dar-csrf");
  const cookies = `${session.raw}; ${csrf.raw}`;

  const whoami = await fetch(`${baseUrl}/api/v1/session`, { headers: { cookie: cookies } });
  assert.equal(whoami.status, 200, output());
  const { user } = await whoami.json();
  return {
    cookies,
    csrfToken: csrf.value,
    user,
    authHeaders: {
      cookie: cookies,
      origin: baseUrl,
      "content-type": "application/json",
      "x-csrf-token": csrf.value,
    },
  };
}

export async function startDarWithOidc(dataRoot, options = {}) {
  const provider = await fakeOidcProvider();
  try {
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const port = await availablePort(), baseUrl = `http://127.0.0.1:${port}`;
      const running = startDar(dataRoot, port, {
        DAR_BASE_URL: baseUrl,
        DAR_OIDC_ISSUER: provider.issuer,
        DAR_OIDC_CLIENT_ID: "dar-test",
        DAR_OIDC_CLIENT_SECRET: "test-secret",
        DAR_DEVELOPMENT: "true",
        DAR_OIDC_ADMIN_CLAIM: "groups",
        DAR_OIDC_ADMIN_VALUE: "dar-admins",
        ...options,
      });
      try {
        await waitForHealth(port, running.child, running.output);
        return { provider, port, baseUrl, running };
      } catch (error) {
        await stopDar(running.child);
        // The probe must release its socket before Dar can bind it. Another
        // isolated process may claim that port in between; only that explicit
        // pre-startup bind failure is eligible for a bounded fresh allocation.
        const collision = running.output().includes('Error: No open ports available') &&
          !running.output().includes('Dar is ready at ');
        if (!collision || attempt === 2 || options.DAR_PORT !== undefined) throw error;
      }
    }
  } catch (error) {
    await provider.close();
    throw error;
  }
}

export async function stopDarWithOidc(context) {
  await stopDar(context.running.child);
  await context.provider.close();
}
