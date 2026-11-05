// Browser verification for the public share visitor page (Stage 7).
// Launches an isolated Dar instance with the hermetic OIDC provider,
// signs in to create shares over the session API, then drives a real
// headless Chromium through the public /share/{token} flows from a
// logged-out context: folder listing, file download, password unlock,
// and a drop upload. Run inside `nix develop`:
//   node test/browser/share-visitor.mjs
import assert from "node:assert/strict";
import { expect } from "@playwright/test";
import { Buffer } from "node:buffer";
import { execFileSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { chromium } from "../helpers/browser.mjs";

import {
  startDarWithOidc,
  stopDarWithOidc,
  waitForHealth,
} from "../helpers/dar-server.mjs";

function chromiumExecutable() {
  try {
    return execFileSync("which", ["chromium"], { encoding: "utf8" }).trim();
  } catch {
    return undefined;
  }
}

const dataRoot = await mkdtemp("/tmp/dar-share-browser-");
// Password-protected shares derive digests from the credential keyring,
// so the instance runs with a deterministic test key.
const credentialKeys =
  `test:${Buffer.from("dar-share-visitor-key-0000000000", "utf8").toString("base64")}`;
const context = await startDarWithOidc(dataRoot, {
  DAR_CREDENTIAL_ENCRYPTION_KEYS: credentialKeys,
});
const consoleErrors = [];
let browser, releaseShare;
async function verifyPublicLayout(page, heading, artifact) {
  for (const theme of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: theme });
    for (const width of [320, 390, 768, 1024, 1440]) {
      await page.setViewportSize({ width, height: width < 768 ? 844 : 1000 });
      await page.getByRole("heading", { name: heading, exact: true }).waitFor();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), true,
        `${artifact} fits ${width}px in ${theme}`);
      const controls = page.locator('button:has(> .sanctuary-icon-action), a:has(> .sanctuary-icon-action)');
      await expect.poll(() => controls.evaluateAll(items => items.flatMap(item => {
        const box = item.getBoundingClientRect(), painted = item.querySelector('use')?.getBBox();
        const name = item.getAttribute('aria-label') || item.querySelector('.sanctuary-visually-hidden')?.textContent;
        return !name?.trim() || box.width < 44 || box.height < 44 || !painted?.width || !painted.height ? [{ name, width: box.width, height: box.height }] : [];
      }))).toEqual([]);
      if (width === 320) await page.screenshot({ path: `/tmp/dar-polish-public-${artifact}-${theme}.png` });
    }
  }
}
try {
  await waitForHealth(context.port, context.running.child, context.running.output);
  browser = await chromium.launch({
    executablePath: chromiumExecutable(),
    headless: true,
  });
  const page = await browser.newPage();
  page.on("console", (message) => {
    const text = message.text();
    // The visitor context never has a session, so its probe's 401 is
    // expected noise; every other console error fails the run.
    if (message.type() === "error" && !text.includes("401 (Unauthorized)")) {
      consoleErrors.push(text);
    }
  });
  page.on("pageerror", (error) => consoleErrors.push(String(error)));

  // Sign in and prepare shared content through the session API so the
  // visitor flows exercise the SPA page rather than setup plumbing.
  await page.goto(context.baseUrl);
  await verifyPublicLayout(page, "Welcome to Dar", "login");
  await page.getByRole("link", { name: "Continue with OpenID Connect" }).click();
  await page.getByRole("heading", { name: /Welcome,/ }).waitFor({ timeout: 15000 });

  const handbookBase64 = Buffer.from("# Shared handbook\n").toString("base64");
  const { tokens, items } = await page.evaluate(async (handbookBase64) => {
    const csrf = document.cookie
      .split("; ")
      .find((pair) => pair.startsWith("dar-csrf="))
      ?.split("=")[1];
    const json = async (method, path, body) => {
      const response = await fetch(path, {
        method,
        credentials: "same-origin",
        headers: {
          "x-csrf-token": csrf ?? "",
          "content-type": "application/json",
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      const parsed = await response.json();
      if (!response.ok) {
        throw new Error(`${method} ${path} -> ${response.status}`);
      }
      return parsed;
    };
    await json("POST", "/api/v1/files/folders", { path: "/Shared Out" });
    const longName = "reading".repeat(24) + ".md";
    const longUpload = await fetch("/api/v1/files/upload?path=" + encodeURIComponent("/Shared Out/" + longName), {
      method: "POST", credentials: "same-origin", headers: { "x-csrf-token": csrf ?? "" }, body: "A shared thought\n",
    });
    if (!longUpload.ok) throw new Error("Long share fixture upload failed");
    const fileShare = await json("POST", "/api/v1/files/shares", { path: "/Shared Out/" + longName });
    await fetch(
      "/api/v1/files/upload?path=" +
        encodeURIComponent("/Shared Out/handbook.md"),
      {
        method: "POST",
        credentials: "same-origin",
        headers: { "x-csrf-token": csrf ?? "" },
        body: Uint8Array.from(atob(handbookBase64), (c) => c.charCodeAt(0)),
      },
    ).then(async (response) => {
      if (!response.ok) throw new Error(`upload -> ${response.status}`);
    });

    const openShare = await json("POST", "/api/v1/files/shares", {
      path: "/Shared Out",
    });
    const passwordShare = await json("POST", "/api/v1/files/shares", {
      path: "/Shared Out",
      password: "orchid-lantern",
    });
    const dropShare = await json("POST", "/api/v1/files/shares", {
      path: "/Shared Out",
      drop: true,
    });
    const listing = await json(
      "GET",
      "/api/v1/files?path=" + encodeURIComponent("/Shared Out"),
    );
    return {
      tokens: {
        open: openShare.token,
        password: passwordShare.token,
        drop: dropShare.token,
        file: fileShare.token,
      },
      longName,
      items: listing.items.map((entry) => entry.name),
    };
  }, handbookBase64);
  assert.equal(items.length, 2); assert.ok(items.includes("handbook.md"));

  // A logged-out visitor sees the shared folder listing.
  const visitor = await browser.newContext();
  const visitorPage = await visitor.newPage();
  visitorPage.on("console", (message) => {
    const text = message.text();
    const url = message.location()?.url ?? "";
    // The visitor context never has a session, so its session probe’s 401
    // is expected noise; the unknown-token check probes a dead share whose
    // 404 is the server’s correct answer. Everything else must stay clean.
    const expected = text.includes("401 (Unauthorized)") ||
      (text.includes("404 (Not Found)") && url.includes("/share/not-a-real-token"));
    if (message.type() === "error" && !expected) {
      consoleErrors.push(text);
    }
  });
  visitorPage.on("pageerror", (error) => consoleErrors.push(String(error)));

  await visitorPage.goto(`${context.baseUrl}/share/${tokens.open}`);
  await visitorPage.getByRole("heading", { name: "Shared Out" }).waitFor({
    timeout: 15000,
  });
  await visitorPage.getByText("handbook.md").waitFor();
  await visitorPage.getByText("Folder", { exact: false }).first().waitFor();
  await verifyPublicLayout(visitorPage, "Shared Out", "folder");

  // The download link serves the shared file's bytes.
  const download = await visitorPage.request.get(
    `${context.baseUrl}/share/${tokens.open}/download?name=` +
      encodeURIComponent("handbook.md"),
  );
  assert.equal(download.status(), 200);
  assert.equal(
    await download.text(),
    "# Shared handbook\n",
    "the shared bytes must round-trip",
  );
  await visitorPage.goto(`${context.baseUrl}/share/${tokens.file}`);
  // Long portable filenames wrap in both the folder rows and file title.
  const longName = items.find(item => item !== "handbook.md");
  await verifyPublicLayout(visitorPage, longName, "file");

  // A password share gates the listing behind the unlock form.
  await visitorPage.goto(`${context.baseUrl}/share/${tokens.password}`);
  await visitorPage.getByRole("heading", { name: "This link is protected" })
    .waitFor({ timeout: 15000 });
  await verifyPublicLayout(visitorPage, "This link is protected", "password");
  await visitorPage.getByRole("textbox", { name: "Password" })
    .fill("wrong-password");
  await visitorPage.getByRole("button", { name: "Open shared link" }).click();
  await visitorPage.getByText("This link needs its password.").waitFor();
  await visitorPage.getByRole("textbox", { name: "Password" })
    .fill("orchid-lantern");
  await visitorPage.getByRole("button", { name: "Open shared link" }).click();
  await visitorPage.getByRole("heading", { name: "Shared Out" }).waitFor({
    timeout: 15000,
  });
  await visitorPage.getByText("handbook.md").waitFor();

  // A drop share offers the upload form and confirms the upload.
  await visitorPage.goto(`${context.baseUrl}/share/${tokens.drop}`);
  await visitorPage.getByRole("heading", { name: "Shared Out" }).waitFor({
    timeout: 15000,
  });
  await visitorPage.getByText("Drop link").waitFor();
  await verifyPublicLayout(visitorPage, "Shared Out", "drop");
  await visitorPage.getByLabel("File")
    .setInputFiles({
      name: "drop-note.txt",
      mimeType: "text/plain",
      buffer: Buffer.from("left at the door\n"),
    });
  await visitorPage.getByRole("button", { name: "Send file" }).click();
  await visitorPage.getByText("Uploaded drop-note.txt.").waitFor();

  // The owner sees the dropped file from their own session.
  await page.getByRole("link", { name: "Files" }).first().click();
  await page.getByRole("button", { name: "Shared Out", exact: true })
    .waitFor();
  await page.getByRole("button", { name: "Shared Out", exact: true }).click();
  await page.getByRole("button", { name: "drop-note.txt", exact: true }).waitFor();

  // An unknown token renders the friendly unavailable screen.
  await visitorPage.goto(`${context.baseUrl}/share/not-a-real-token`);
  await visitorPage.getByRole("heading", { name: "This link is unavailable" })
    .waitFor({ timeout: 15000 });
  await verifyPublicLayout(visitorPage, "This link is unavailable", "unavailable");

  // Browser history may leave a public page while its JSON reply is held.
  // Its own context must cancel the request before a late result can render.
  await visitorPage.goto(context.baseUrl + "/login");
  await visitorPage.getByRole("heading", { name: "Welcome to Dar", exact: true }).waitFor();
  let shareEntered, heldRequest, shareCancelled = false;
  const started = new Promise(resolve => { shareEntered = resolve; });
  const heldShare = new Promise(resolve => { releaseShare = resolve; });
  await visitorPage.route(`${context.baseUrl}/share/${tokens.open}`, async route => {
    const response = await route.fetch(); heldRequest = route.request(); shareEntered();
    await heldShare;
    try { await route.fulfill({ response }); }
    catch (error) {
      if (!shareCancelled || !/Route is already handled/.test(error.message)) throw error;
    }
  });
  await visitorPage.evaluate(path => {
    history.pushState(null, "", path); dispatchEvent(new PopStateEvent("popstate"));
  }, `/share/${tokens.open}`);
  await started;
  const cancelled = visitorPage.waitForEvent("requestfailed", { predicate: request => request === heldRequest });
  await visitorPage.goBack();
  await visitorPage.getByRole("heading", { name: "Welcome to Dar", exact: true }).waitFor();
  assert.match((await cancelled).failure().errorText, /ABORTED|CANCELLED/);
  shareCancelled = true;
  releaseShare();
  await visitorPage.unroute(`${context.baseUrl}/share/${tokens.open}`);

  assert.deepEqual(consoleErrors, [], "console must stay clean");
  console.log("share visitor browser verification passed");
} finally {
  releaseShare?.();
  if (browser) await browser.close();
  await stopDarWithOidc(context);
  await rm(dataRoot, { recursive: true, force: true });
}
