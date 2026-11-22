// Credentials in the formats Dar stores them, for tests that write accounts, sessions or old
// records directly: scrypt password verifiers and SHA-256 token digests.
import { createHash, randomBytes, scrypt } from "node:crypto";

const prefix = "scrypt$131072$8$1";
const options = { N: 131072, r: 8, p: 1, maxmem: 160 * 1024 * 1024 };

/** A password verifier exactly as the runtime makes one. */
export async function hashPassword(password: string): Promise<string> {
  if ([...password].length < 15 || Buffer.byteLength(password, "utf8") > 1024) throw new Error("Use at least 15 characters and no more than 1024 UTF-8 bytes.");
  const salt = randomBytes(16);
  const digest = await new Promise<Buffer>((resolve, reject) => scrypt(password, salt, 32, options, (error, result) => error ? reject(error) : resolve(result)));
  return `${prefix}$${salt.toString("base64url")}$${digest.toString("base64url")}`;
}

export function digestToken(secret: string): string {
  return createHash("sha256").update(secret, "utf8").digest("hex");
}

/** A random secret and the digest that is stored for it. */
export function createOpaqueToken(): { secret: string; digest: string } {
  const secret = randomBytes(32).toString("base64url");
  return { secret, digest: digestToken(secret) };
}

/** A procedure's JSON result; `{error:{code}}` becomes an error with the code. */
export function readProcedure<T>(value: string): T {
  const result = JSON.parse(value);
  if (result?.error) throw new Error(result.error.code ?? "The request could not be completed.");
  return result as T;
}

/** The instance's identity signing key, read from its sealed file (for tests that forge tokens). */
export async function signingJwk(secrets: string): Promise<import("jose").JWK> {
  const { readFile } = await import("node:fs/promises"), { createDecipheriv } = await import("node:crypto"), path = await import("node:path");
  const key = Buffer.from((await readFile(path.join(secrets, "identity-keyring.key"), "utf8")).trim(), "base64url");
  const envelope = JSON.parse(await readFile(path.join(secrets, "signing-key.json"), "utf8"));
  if (envelope.format !== "dar-keyring-v1") throw new Error("Unrecognized signing key.");
  const decipher = createDecipheriv("aes-256-gcm", key, Buffer.from(envelope.iv, "base64url"));
  decipher.setAAD(Buffer.from("dar.identity.signing-key.v1")); decipher.setAuthTag(Buffer.from(envelope.tag, "base64url"));
  return JSON.parse(Buffer.concat([decipher.update(Buffer.from(envelope.ciphertext, "base64url")), decipher.final()]).toString("utf8"));
}

/** A browser token for a session, signed as the runtime's issuer signs them (`/auth/issuer`). */
export async function browserToken(secrets: string, issuer: string, sessionId: string, expiresAt: number) {
  const { importJWK, SignJWT } = await import("jose"), { randomUUID } = await import("node:crypto");
  const jwk = await signingJwk(secrets), key = await importJWK(jwk, "ES256");
  return new SignJWT({ sid: sessionId }).setProtectedHeader({ alg: "ES256", kid: jwk.kid })
    .setIssuer(issuer).setAudience("dar-browser").setSubject(`${sessionId}:${randomUUID()}`).setIssuedAt()
    .setExpirationTime(Math.min(Math.floor(expiresAt / 1000), Math.floor(Date.now() / 1000) + 300)).sign(key);
}
