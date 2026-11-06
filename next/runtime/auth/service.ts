import { randomUUID } from "node:crypto";
import type { Database } from "../database.ts";
import { AppError } from "../errors.ts";
import { hashPassword, verifyPassword, validateNewPassword } from "./password.ts";
import { createOpaqueToken, digestToken } from "./session.ts";
import type { createIssuer } from "./issuer.ts";

export interface User { id: string; username: string; displayName: string; administrator: boolean }
interface Account extends User { verifier: string; enabled: boolean }
export interface Authenticated {
  session: { id: string; userId: string; digest: string; csrfDigest: string; expiresAt: number; active: boolean };
  user: User;
}
export type Issuer = Awaited<ReturnType<typeof createIssuer>>;

/** Hash work is bounded independently of the HTTP connection limit. */
export async function authService(db: Database, issuer: Issuer) {
  const dummy = await hashPassword("dummy-credential-not-an-account");
  let hashing = 0;
  async function bounded<T>(work: () => Promise<T>): Promise<T> {
    if (hashing >= 2) throw new AppError("rateLimited", 429, "Try signing in again shortly.");
    hashing++;
    try { return await work(); } finally { hashing--; }
  }
  return {
    async status() { return db.call<{ initialized: boolean }>("service_status"); },
    async createAccount(username: string, displayName: string, password: string, first: boolean) {
      try { validateNewPassword(password); } catch { throw new AppError("invalidPassword", 400, "Use at least 15 characters, up to 1024 UTF-8 bytes."); }
      const verifier = await bounded(() => hashPassword(password));
      const id = randomUUID();
      await db.call("service_create_account", [id, username.toLowerCase(), displayName, verifier, first]);
      return id;
    },
    async login(username: string, password: string) {
      const account = await db.call<Account | null>("service_account", [username.toLowerCase()]);
      const valid = await bounded(() => verifyPassword(password, account?.verifier || dummy));
      if (!valid || !account?.enabled) throw new AppError("invalidCredentials", 401, "The username or password is incorrect.");
      const token = createOpaqueToken();
      const csrf = issuer.csrf(token.secret);
      const session = { id: randomUUID(), userId: account.id, digest: token.digest, csrfDigest: digestToken(csrf), expiresAt: Date.now() + 7 * 86400_000 };
      await db.call("service_create_session", [session.id, session.userId, session.digest, session.csrfDigest, session.expiresAt, account.verifier]);
      return { secret: token.secret, csrf, user: { id: account.id, username: account.username, displayName: account.displayName, administrator: account.administrator } };
    },
    async authenticate(secret: string | undefined): Promise<Authenticated | null> {
      if (!secret || !/^[A-Za-z0-9_-]{43}$/.test(secret)) return null;
      return db.call<Authenticated | null>("service_session", [digestToken(secret)]);
    },
    async confirm(auth: Authenticated, password: string) {
      const account=await db.call<Account>("service_account",[auth.user.username]);
      if(!await bounded(()=>verifyPassword(password,account?.verifier||dummy)))throw new AppError("invalidCredentials",401);
      return account.verifier;
    },
    async provisionPassword(password: string) {
      try { validateNewPassword(password); } catch { throw new AppError("invalidPassword",400,"Use at least 15 characters, up to 1024 UTF-8 bytes."); }
      return bounded(()=>hashPassword(password));
    },
    async changePassword(auth: Authenticated, current: string, replacement: string) {
      try { validateNewPassword(replacement); } catch { throw new AppError("invalidPassword", 400, "Use at least 15 characters, up to 1024 UTF-8 bytes."); }
      const account = await db.call<Account>("service_account", [auth.user.username]);
      const verifier = await bounded(async () => {
        if (!await verifyPassword(current, account.verifier)) throw new AppError("invalidCredentials", 401);
        return hashPassword(replacement);
      });
      await db.call("service_change_password", [auth.user.id, verifier, account.verifier]);
    },
    async logout(auth: Authenticated) { await db.call("service_revoke_session", [auth.session.id]); },
  };
}
