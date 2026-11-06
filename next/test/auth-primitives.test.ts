import assert from "node:assert/strict";
import { test } from "node:test";
import { hashPassword, validateNewPassword, verifyPassword } from "../runtime/auth/password.ts";
import { authorizeCookieWrite, createOpaqueToken, digestToken, isActiveSession, matchesToken } from "../runtime/auth/session.ts";

test("password verifiers are salted and preserve whitespace and Unicode", async () => {
  const password = "  a long café passphrase  ";
  const first = await hashPassword(password);
  const second = await hashPassword(password);
  assert.notEqual(first, second);
  assert.equal(await verifyPassword(password, first), true);
  assert.equal(await verifyPassword(password.trim(), first), false);
  assert.equal(await verifyPassword(password.normalize("NFD"), first), false);
});

test("password bounds use characters for minimum and bytes for maximum", () => {
  assert.throws(() => validateNewPassword("a".repeat(14)));
  assert.doesNotThrow(() => validateNewPassword("😀".repeat(15)));
  assert.doesNotThrow(() => validateNewPassword("a".repeat(1024)));
  assert.throws(() => validateNewPassword("😀".repeat(257)));
});

test("malformed verifiers and attacker-selected work factors fail closed", async () => {
  for (const verifier of ["", "scrypt$999999999$8$1$salt$hash", "scrypt$131072$8$1$bad$bad"]) {
    assert.equal(await verifyPassword("a sufficiently long password", verifier), false);
  }
  assert.equal(await verifyPassword("a".repeat(1025), "malformed"), false);
});

test("session and CSRF tokens are random secrets persisted only as digests", () => {
  const token = createOpaqueToken();
  assert.notEqual(token.secret, createOpaqueToken().secret);
  assert.notEqual(token.secret, token.digest);
  assert.equal(digestToken(token.secret), token.digest);
  assert.equal(matchesToken(token.secret, token.digest), true);
  assert.equal(matchesToken(createOpaqueToken().secret, token.digest), false);
  assert.equal(matchesToken(token.secret, "malformed"), false);
});

test("expiry and revocation invalidate sessions at the boundary", () => {
  const record = { userId: "user", expiresAt: 100, revokedAt: null };
  assert.equal(isActiveSession(record, 99), true);
  assert.equal(isActiveSession(record, 100), false);
  assert.equal(isActiveSession({ ...record, revokedAt: 0 }, 99), false);
  assert.equal(isActiveSession({ ...record, expiresAt: NaN }, 99), false);
});

test("cookie writes reject missing/cross-site origins and non-session CSRF", () => {
  const token = createOpaqueToken();
  const origin = "https://cloud.example";
  assert.equal(authorizeCookieWrite(origin, origin, token.secret, token.digest), true);
  for (const supplied of [undefined, "null", "https://cloud.example.attacker.test", "http://cloud.example"]) {
    assert.equal(authorizeCookieWrite(origin, supplied, token.secret, token.digest), false);
  }
  assert.equal(authorizeCookieWrite(origin, origin, undefined, token.digest), false);
  assert.equal(authorizeCookieWrite(origin, origin, createOpaqueToken().secret, token.digest), false);
});
