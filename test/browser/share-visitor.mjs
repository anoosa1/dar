// Browser verification for the public share visitor page (Stage 7).
// Launches an isolated Dar instance with the hermetic OIDC provider,
// signs in to create shares over the session API, then drives a real
// headless Chromium through the public /share/{token} flows from a
// logged-out context: folder listing, file download, password unlock,
// and a drop upload. Run inside `nix develop`:
//   node test/browser/share-visitor.mjs
import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { execFileSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { chromium } from "playwright";

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
let browser;
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
      },
      items: listing.items.map((entry) => entry.name),
    };
  }, handbookBase64);
  assert.deepEqual(items, ["handbook.md"]);

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

  // A password share gates the listing behind the unlock form.
  await visitorPage.goto(`${context.baseUrl}/share/${tokens.password}`);
  await visitorPage.getByRole("heading", { name: "This link is protected" })
    .waitFor({ timeout: 15000 });
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
  await page.getByText("drop-note.txt").waitFor();

  // An unknown token renders the friendly unavailable screen.
  await visitorPage.goto(`${context.baseUrl}/share/not-a-real-token`);
  await visitorPage.getByRole("heading", { name: "This link is unavailable" })
    .waitFor({ timeout: 15000 });

  assert.deepEqual(consoleErrors, [], "console must stay clean");
  console.log("share visitor browser verification passed");
} finally {
  if (browser) await browser.close();
  await stopDarWithOidc(context);
  await rm(dataRoot, { recursive: true, force: true });
}
