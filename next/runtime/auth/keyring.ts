import { createCipheriv, createDecipheriv, randomBytes, randomUUID } from "node:crypto";
import { lstat, readFile } from "node:fs/promises";
import { generateKeyPair, exportJWK, type JWK } from "jose";
import path from "node:path";
import { createDurableFile } from "../atomic.ts";

const associatedData = Buffer.from("dar.identity.signing-key.v1");
async function privateText(filename: string) {
  const stat = await lstat(filename);
  if (!stat.isFile() || (stat.mode & 0o077)) throw new Error("Identity key files must be regular files private to their owner.");
  return readFile(filename, "utf8");
}
async function masterKey(directory: string, allowCreate = false) {
  const filename = path.join(directory, "identity-keyring.key");
  let encoded: string;
  try { encoded = await privateText(filename); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT" || !allowCreate) throw error;
    encoded = randomBytes(32).toString("base64url"); await createDurableFile(filename, encoded);
  }
  if (!/^[A-Za-z0-9_-]{43}$/.test(encoded)) throw new Error("Identity keyring is corrupt.");
  return Buffer.from(encoded, "base64url");
}

/** Private JWK material is encrypted at rest; callers keep it in server memory only. */
export async function signingJwk(directory: string): Promise<JWK> {
  const filename = path.join(directory, "signing-key.json");
  let encoded: string;
  try { encoded = await privateText(filename); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    const key = await masterKey(directory, true), iv = randomBytes(12);
    const { privateKey } = await generateKeyPair("ES256", { extractable: true });
    const jwk = { ...await exportJWK(privateKey), kid: `dar-identity-${randomUUID()}`, alg: "ES256", use: "sig" };
    const cipher = createCipheriv("aes-256-gcm", key, iv); cipher.setAAD(associatedData);
    const ciphertext = Buffer.concat([cipher.update(JSON.stringify(jwk), "utf8"), cipher.final()]);
    await createDurableFile(filename, JSON.stringify({ format: "dar-keyring-v1", iv: iv.toString("base64url"), ciphertext: ciphertext.toString("base64url"), tag: cipher.getAuthTag().toString("base64url") }));
    return jwk;
  }
  try {
    const envelope = JSON.parse(encoded);
    if (envelope.format !== "dar-keyring-v1") throw new Error();
    const key = await masterKey(directory);
    const decipher = createDecipheriv("aes-256-gcm", key, Buffer.from(envelope.iv, "base64url"));
    decipher.setAAD(associatedData); decipher.setAuthTag(Buffer.from(envelope.tag, "base64url"));
    return JSON.parse(Buffer.concat([decipher.update(Buffer.from(envelope.ciphertext, "base64url")), decipher.final()]).toString("utf8"));
  } catch { throw new Error("The identity signing key cannot be decrypted. Restore the matching keyring and signing key."); }
}

/** Purpose-bound boxes let private provider state live in SpacetimeDB without
 * placing bearer material or client secrets there as plaintext. */
export async function keyringBox(directory: string, purpose: string) {
  const key=await masterKey(directory),aad=Buffer.from(`dar.identity.${purpose}.v1`);
  return {
    encrypt(value:unknown){const iv=randomBytes(12),cipher=createCipheriv("aes-256-gcm",key,iv);cipher.setAAD(aad);const data=Buffer.concat([cipher.update(JSON.stringify(value),"utf8"),cipher.final()]);return JSON.stringify({iv:iv.toString("base64url"),tag:cipher.getAuthTag().toString("base64url"),data:data.toString("base64url")});},
    decrypt<T>(encoded:string):T {try{const p=JSON.parse(encoded),decipher=createDecipheriv("aes-256-gcm",key,Buffer.from(p.iv,"base64url"));decipher.setAAD(aad);decipher.setAuthTag(Buffer.from(p.tag,"base64url"));return JSON.parse(Buffer.concat([decipher.update(Buffer.from(p.data,"base64url")),decipher.final()]).toString("utf8"));}catch{throw new Error("The identity payload cannot be decrypted.");}}
  };
}
