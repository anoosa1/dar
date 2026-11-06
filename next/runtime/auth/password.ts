import { randomBytes, scrypt, timingSafeEqual } from "node:crypto";

// Versioned, fixed work factors prevent stored verifier parameters from becoming
// an allocation primitive. Password hashing must run behind a bounded login queue.
const prefix = "scrypt$131072$8$1";
const options = { N: 131072, r: 8, p: 1, maxmem: 160 * 1024 * 1024 };
const maxPasswordBytes = 1024;

function derive(password: string, salt: Buffer): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    scrypt(password, salt, 32, options, (error, result) => {
      if (error) reject(error);
      else resolve(result);
    });
  });
}

export function validateNewPassword(password: string): void {
  if ([...password].length < 15 || Buffer.byteLength(password, "utf8") > maxPasswordBytes) {
    throw new Error("Use at least 15 characters and no more than 1024 UTF-8 bytes.");
  }
}

/** Hash the exact supplied password: spaces and Unicode are never normalized. */
export async function hashPassword(password: string): Promise<string> {
  validateNewPassword(password);
  const salt = randomBytes(16);
  const digest = await derive(password, salt);
  return `${prefix}$${salt.toString("base64url")}$${digest.toString("base64url")}`;
}

export async function verifyPassword(password: string, verifier: string): Promise<boolean> {
  if (Buffer.byteLength(password, "utf8") > maxPasswordBytes) return false;
  const parts = verifier.split("$");
  if (parts.length !== 6 || parts.slice(0, 4).join("$") !== prefix) return false;
  const salt = parts[4];
  const expected = parts[5];
  if (!/^[A-Za-z0-9_-]{22}$/.test(salt) || !/^[A-Za-z0-9_-]{43}$/.test(expected)) return false;
  const saltBytes = Buffer.from(salt, "base64url");
  const expectedBytes = Buffer.from(expected, "base64url");
  if (saltBytes.toString("base64url") !== salt || expectedBytes.toString("base64url") !== expected) return false;
  return timingSafeEqual(await derive(password, saltBytes), expectedBytes);
}
