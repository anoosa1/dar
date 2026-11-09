import { readFile } from "node:fs/promises";
import path from "node:path";
import { isDeepStrictEqual } from "node:util";
import { clientMetadata } from "./http.ts";
import type { IdentityProvider } from "./provider.ts";
import type { Document } from "./scim-model.ts";

/** Validate the whole file before applying revisioned client updates at startup. */
export async function configureOidc(filename: string, store: IdentityProvider["store"], development: boolean, optional = false) {
  let source: string;
  try { source = await readFile(filename, "utf8"); }
  catch (error) { if (optional && (error as NodeJS.ErrnoException).code === "ENOENT") return; throw error; }
  let input: unknown;
  try { input = JSON.parse(source); } catch { throw new Error("Invalid OIDC configuration JSON."); }
  if (!input || typeof input !== "object" || !("clients" in input) || !Array.isArray(input.clients) || input.clients.length > 200) throw new Error("OIDC configuration requires a clients array (up to 200 clients).");
  const ids = new Set<string>();
  const clients = [];
  for (const value of input.clients) {
    if (!value || typeof value !== "object") throw new Error("Invalid OIDC client configuration.");
    const p = value as Document, id = p.id;
    if (typeof id !== "string" || !/^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$/.test(id) || ids.has(id)) throw new Error("OIDC client IDs must be unique URL-safe identifiers.");
    ids.add(id);
    if (p.enabled !== undefined && typeof p.enabled !== "boolean") throw new Error("Invalid OIDC client enabled setting.");
    if (Object.keys(p).some(key => !["id", "name", "type", "redirectUris", "postLogoutRedirectUris", "scopes", "enabled", "secretFile"].includes(key))) throw new Error("Unknown OIDC client setting; use secretFile for secrets.");
    let secret: string | undefined;
    if (p.type === "confidential") {
      if (typeof p.secretFile !== "string" || !p.secretFile) throw new Error("Confidential OIDC clients require secretFile.");
      secret = (await readFile(path.resolve(path.dirname(filename), p.secretFile), "utf8")).replace(/\r?\n$/, "");
      if (secret.length < 32 || /[\r\n]/.test(secret)) throw new Error("OIDC client secrets must have at least 32 characters and no embedded newlines.");
    } else if (p.secretFile !== undefined) throw new Error("Public OIDC clients cannot have secretFile.");
    const metadata = clientMetadata(p, id, development, secret);
    clients.push({ id, metadata, enabled: p.enabled !== false });
  }
  for (const client of clients) {
    const old = await store.call<{ revision: number; encrypted: string; enabled: boolean } | null>("client", { id: client.id });
    if (old && old.enabled === client.enabled && isDeepStrictEqual(store.clientBox.decrypt(old.encrypted), client.metadata)) continue;
    const { client_secret: _secret, ...metadata } = client.metadata;
    await store.call("saveClient", { id: client.id, revision: old?.revision ?? 0, enabled: client.enabled, metadata: JSON.stringify(metadata), encrypted: store.clientBox.encrypt(client.metadata) });
  }
}
