// Isolated Dar instances for integration tests: their own database and root, on the stack
// under test. DAR_TEST_STACK=rust (set by rust/verify.mjs) runs the Rust module and runtime
// (DAR_RUST_MODULE, DAR_RUST_RUNTIME, DAR_RUST_CLIENT); otherwise the TypeScript ones run
// in-process. Tests use the same calls either way.
import assert from "node:assert/strict";
import {spawn, spawnSync, type ChildProcess} from "node:child_process";
import {createServer, type AddressInfo} from "node:net";
import path from "node:path";
import {startServer} from "../runtime/server.ts";

export const stack = process.env.DAR_TEST_STACK === "rust" ? "rust" : "typescript";

export type InstanceConfig = {
  origin?: string; root: string; databaseName: string; port?: number; development?: boolean;
  chatBridge?: string; newsAllowPrivateNetworks?: boolean; immichAllowLocalHost?: boolean;
  codexFile?: string; claudeFile?: string; appsFile?: string; oidcFile?: string;
  /** Another database server and its tokens (defaults: the fixture's). */
  databaseUrl?: string; serviceTokenFile?: string; bootstrapTokenFile?: string;
};
export type Instance = { base: string; port: number; config: InstanceConfig; close(): Promise<void>; restart(): Promise<void> };

export async function freePort() {
  const listener = createServer(); await new Promise<void>(resolve => listener.listen(0, "127.0.0.1", resolve));
  const {port} = listener.address() as AddressInfo; await new Promise<void>(resolve => listener.close(() => resolve())); return port;
}

/** Publishes the stack's module as a new database named `name`, trusting `origin`'s issuer. */
export function moduleArguments() {
  return stack === "rust" ? ["--bin-path", process.env.DAR_RUST_MODULE!] : ["--js-path", path.resolve(import.meta.dirname, "../module/dist/bundle.js")];
}
export function publishModule(name: string, origin: string) {
  const module = moduleArguments();
  const published = spawnSync(process.env.DAR_SPACETIME_BIN!, ["--root-dir", process.env.DAR_SPACETIME_CLI_ROOT!, "publish", name, ...module, "--server", process.env.DAR_DATABASE_URL!, "--yes", "--no-config"],
    {env: {...process.env, DAR_ISSUER: `${origin}/auth/issuer`}, encoding: "utf8", timeout: 60_000});
  assert.equal(published.status, 0, published.stdout + published.stderr);
}

function environment(config: InstanceConfig, port: number, base: string): NodeJS.ProcessEnv {
  const flag = (value?: boolean) => value ? "1" : "0";
  const optional = Object.fromEntries(Object.entries({DAR_CHAT_BRIDGE: config.chatBridge, DAR_CODEX_FILE: config.codexFile, DAR_CLAUDE_FILE: config.claudeFile,
    DAR_APPS_FILE: config.appsFile, DAR_OIDC_FILE: config.oidcFile}).filter(([, value]) => value !== undefined));
  return {...process.env, ...optional, DAR_HOST: "127.0.0.1", DAR_PORT: String(port), DAR_ORIGIN: base, DAR_ROOT: config.root,
    DAR_DATABASE_URL: config.databaseUrl ?? process.env.DAR_DATABASE_URL, DAR_DATABASE_NAME: config.databaseName, DAR_DATABASE_TOKEN_FILE: config.serviceTokenFile ?? process.env.DAR_DATABASE_TOKEN_FILE,
    DAR_BOOTSTRAP_TOKEN_FILE: config.bootstrapTokenFile ?? process.env.DAR_BOOTSTRAP_TOKEN_FILE, DAR_DEVELOPMENT: flag(config.development ?? true),
    DAR_NEWS_ALLOW_PRIVATE_NETWORKS: flag(config.newsAllowPrivateNetworks), DAR_IMMICH_ALLOW_LOCAL_HOST: flag(config.immichAllowLocalHost),
    DAR_ISSUER: `${base}/auth/issuer`, DAR_CLIENT_DIST: process.env.DAR_RUST_CLIENT};
}

async function ready(base: string, child: ChildProcess, log: () => string) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) throw new Error(`The Rust runtime stopped during startup.\n${log()}`);
    try { if ((await fetch(`${base}/api/v1/auth/status`, {signal: AbortSignal.timeout(500)})).ok) return; } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  throw new Error(`The Rust runtime did not become ready.\n${log()}`);
}

/** Starts the stack's runtime on an existing database. Port 0 picks a free port. */
export async function startInstance(config: InstanceConfig): Promise<Instance> {
  const port = config.port || await freePort(), base = config.origin ?? `http://127.0.0.1:${port}`;
  let stopper: () => Promise<void>;
  async function launch() {
    if (stack === "rust") {
      let output = "";
      const child = spawn(process.env.DAR_RUST_RUNTIME!, [], {env: environment(config, port, base), stdio: ["ignore", "pipe", "pipe"]});
      for (const stream of [child.stdout!, child.stderr!]) stream.on("data", chunk => { output = (output + chunk).slice(-16_000); });
      await ready(`http://127.0.0.1:${port}`, child, () => output);
      stopper = () => new Promise<void>(resolve => {
        if (child.exitCode !== null || child.signalCode !== null) return resolve();
        const timer = setTimeout(() => child.kill("SIGKILL"), 15_000);
        child.once("exit", () => { clearTimeout(timer); resolve(); }); child.kill("SIGTERM");
      });
    } else {
      const runtime = await startServer({host: "127.0.0.1", port, origin: base, root: config.root, databaseUrl: config.databaseUrl ?? process.env.DAR_DATABASE_URL!, databaseName: config.databaseName,
        serviceTokenFile: config.serviceTokenFile ?? process.env.DAR_DATABASE_TOKEN_FILE!, bootstrapTokenFile: config.bootstrapTokenFile ?? process.env.DAR_BOOTSTRAP_TOKEN_FILE!, development: config.development ?? true,
        chatBridge: config.chatBridge, newsAllowPrivateNetworks: config.newsAllowPrivateNetworks, immichAllowLocalHost: config.immichAllowLocalHost,
        codexFile: config.codexFile, claudeFile: config.claudeFile, appsFile: config.appsFile, oidcFile: config.oidcFile});
      stopper = () => runtime.close();
    }
  }
  await launch();
  return {base, port, config, close: () => stopper(), async restart() { await stopper(); await launch(); }};
}

/** A fresh database and root: publishes the module, then starts the runtime. */
export async function isolatedInstance(root: string, prefix: string, options: Omit<InstanceConfig, "root" | "databaseName"> & {stateDirectory?: boolean} = {}) {
  const databaseName = `${prefix}-${crypto.randomUUID().slice(0, 8)}`, port = options.port || await freePort(), base = options.origin ?? `http://127.0.0.1:${port}`;
  publishModule(databaseName, base);
  return startInstance({...options, port, origin: base, databaseName, root: options.stateDirectory === false ? root : path.join(root, "state")});
}
