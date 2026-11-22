// The Rust runtime's operator commands (`dar-runtime <command>`), run against a test's own
// database and root: backups, restore, exports, installed apps and on-demand maintenance.
import { spawnSync } from "node:child_process";
import type { Database } from "./database.ts";

function runtime() {
  const binary = process.env.DAR_RUST_RUNTIME;
  if (!binary) throw new Error("DAR_RUST_RUNTIME names the Rust runtime (rust/verify.mjs sets it).");
  return binary;
}

/** Runs `dar-runtime <args>` for `db` (and `root` when the command needs one); returns its output. */
export function operator(db: Database | undefined, root: string | undefined, ...args: string[]): string {
  const env: NodeJS.ProcessEnv = { ...process.env };
  if (db) Object.assign(env, { DAR_DATABASE_URL: db.url, DAR_DATABASE_NAME: db.name, DAR_DATABASE_TOKEN_FILE: db.tokenFile });
  if (root) env.DAR_ROOT = root;
  const result = spawnSync(runtime(), args, { env, encoding: "utf8", timeout: 120_000 });
  if (result.status !== 0) throw new Error((result.stderr || result.stdout || `dar-runtime ${args[0]} failed`).trim());
  return result.stdout.trim();
}

/** Delivers due reminders now, as the runtime's periodic worker does. */
export async function deliverReminders(db: Database) { operator(db, undefined, "deliver-reminders"); }

export async function backup(db: Database, root: string, destination: string) { return JSON.parse(operator(db, root, "backup", destination)) as { id: string; blobs: number }; }
export async function restore(db: Database, source: string, root: string) { return JSON.parse(operator(db, root, "restore", source)) as { blobs: number }; }
export async function exportFiles(db: Database, root: string, username: string, destination: string) { return JSON.parse(operator(db, root, "export-files", username, destination)) as { files: number }; }
export async function reconcileStorage(db: Database, root: string) { return JSON.parse(operator(db, root, "reconcile-storage")).count as number; }
export async function sweepOrphans(db: Database, root: string) { return JSON.parse(operator(db, root, "sweep-orphans")).count as number; }

/** `apps-install|apps-enable|apps-disable|apps-uninstall <root> [argument]`, offline. */
export async function manageApps(root: string, command: string, argument?: string) { return operator(undefined, undefined, command, root, ...(argument ? [argument] : [])); }

/** Applies an OIDC client file to the database, as the runtime does with DAR_OIDC_FILE at startup. */
export async function configureOidc(filename: string, optional = false) {
  const db = { url: process.env.DAR_DATABASE_URL!, name: process.env.DAR_DATABASE_NAME!, tokenFile: process.env.DAR_DATABASE_TOKEN_FILE! } as Database;
  operator(db, process.env.DAR_TEST_ROOT, "configure-oidc", filename, ...(optional ? ["optional"] : []));
}

/** The provider's own records (`service_provider`), as its store reads and writes them. */
export function provider(db: Database) {
  return { call: <T>(operation: string, payload: object = {}) => db.call<T>("service_provider", [operation, JSON.stringify(payload)]) };
}
