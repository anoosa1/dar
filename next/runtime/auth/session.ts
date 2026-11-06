import { createHash, randomBytes, timingSafeEqual } from "node:crypto";

/** Only the digest is persisted; return the secret once to its authenticated owner. */
export function createOpaqueToken(): { secret: string; digest: string } {
  const secret = randomBytes(32).toString("base64url");
  return { secret, digest: digestToken(secret) };
}

export function digestToken(secret: string): string {
  return createHash("sha256").update(secret, "utf8").digest("hex");
}

export function matchesToken(secret: string, digest: string): boolean {
  if (!/^[A-Za-z0-9_-]{43}$/.test(secret) || !/^[a-f0-9]{64}$/.test(digest)) return false;
  return timingSafeEqual(Buffer.from(digestToken(secret), "hex"), Buffer.from(digest, "hex"));
}

export interface SessionRecord {
  userId: string;
  expiresAt: number;
  revokedAt: number | null;
}

export function isActiveSession(session: SessionRecord, now: number): boolean {
  return session.revokedAt === null && Number.isFinite(session.expiresAt) && session.expiresAt > now;
}

/** Cookie-authenticated writes require both exact origin and a session-bound CSRF token. */
export function authorizeCookieWrite(
  expectedOrigin: string,
  requestOrigin: string | undefined,
  csrfToken: string | undefined,
  csrfDigest: string,
): boolean {
  return requestOrigin === expectedOrigin && csrfToken !== undefined && matchesToken(csrfToken, csrfDigest);
}
