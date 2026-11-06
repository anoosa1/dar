import { createHmac, randomUUID } from "node:crypto";
import { AppError } from "../errors.ts";
import { importJWK, SignJWT } from "jose";
import { signingJwk } from "./keyring.ts";

export async function createIssuer(directory: string, issuer: string) {
  const jwk = await signingJwk(directory);
  const key = await importJWK(jwk, "ES256");
  const { d: _privatePart, ...publicKey } = jwk;
  return {
    issuer,
    jwks: { keys: [publicKey] },
    csrf(secret: string) { return createHmac("sha256", jwk.d!).update(`csrf:${secret}`).digest("base64url"); },
    async token(sessionId: string, expiresAt: number, tabId: string = randomUUID()) {
      if (!/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(tabId)) throw new AppError("invalidInput");
      return new SignJWT({ sid: sessionId }).setProtectedHeader({ alg: "ES256", kid: jwk.kid })
        .setIssuer(issuer).setAudience("dar-browser").setSubject(`${sessionId}:${tabId}`).setIssuedAt()
        .setExpirationTime(Math.min(Math.floor(expiresAt / 1000), Math.floor(Date.now() / 1000) + 300)).sign(key);
    },
  };
}
