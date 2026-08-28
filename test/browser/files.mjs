// Manual browser verification for the Files SPA (Stage 1). Launches an
// isolated Dar instance with the hermetic OIDC provider, drives a real
// headless Chromium through the Files page flows, and fails on any console
// error. Run inside `nix develop` so the packaged Chromium is available:
//   node test/browser/files.mjs
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

// Prefer the nix-provided Chromium (its system libraries are bundled);
// fall back to Playwright's own download for environments that have it.
function chromiumExecutable() {
  try {
    return execFileSync("which", ["chromium"], { encoding: "utf8" }).trim();
  } catch {
    return undefined;
  }
}

const dataRoot = await mkdtemp("/tmp/dar-files-browser-");
const context = await startDarWithOidc(dataRoot);
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
    // The session probe legitimately returns 401 before sign-in; the
    // browser logs it as a resource error. Anything else must stay clean.
    if (message.type() === "error" && !text.includes("401 (Unauthorized)")) {
      consoleErrors.push(text);
    }
  });
  page.on("pageerror", (error) => consoleErrors.push(String(error)));

  await page.goto(context.baseUrl);
  await page.getByRole("link", { name: "Continue with OpenID Connect" }).click();
  await page.getByRole("heading", { name: /Welcome,/ }).waitFor({ timeout: 15000 });

  // The Files page lists an empty folder before anything is created.
  await page.getByRole("link", { name: "Files" }).first().click();
  await page.getByRole("heading", { name: "Your files" }).waitFor({ timeout: 15000 });
  await page.getByText("This folder is empty.").waitFor();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Home").waitFor();

  // Create a folder and open it through the breadcrumb-aware row.
  await page.getByRole("button", { name: "New folder" }).click();
  await page.getByRole("textbox", { name: "New folder name" }).fill("Browser Docs");
  await page.getByRole("button", { name: "Create" }).click();
  await page.getByRole("button", { name: "Browser Docs", exact: true }).waitFor();
  await page.getByRole("button", { name: "Browser Docs", exact: true }).click();
  await page.getByText("This folder is empty.").waitFor();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Browser Docs").waitFor();

  // Create an empty file and favorite it.
  await page.getByRole("button", { name: "New file" }).click();
  await page.getByRole("textbox", { name: "New file name" }).fill("welcome.md");
  await page.getByRole("button", { name: "Create" }).click();
  await page.getByText("0 B · text/markdown").waitFor();
  await page.getByRole("button", { name: "Add welcome.md to favorites" }).click();
  await page.getByRole("button", { name: "Remove welcome.md from favorites" }).waitFor();

  // Upload replaces the empty file's content through the picker input.
  await page.getByLabel("Upload file").setInputFiles({
    name: "welcome.md",
    mimeType: "text/markdown",
    buffer: Buffer.from("# Hello from the browser\n"),
  });
  await page.getByText("25 B · text/markdown").waitFor();

  // Delete with the inline confirmation, then verify the row is gone.
  await page.getByRole("button", { name: "Send welcome.md to trash" }).click();
  await page.getByRole("button", { name: "Confirm delete" }).click();
  await page.getByText("This folder is empty.").waitFor();

  // The folder listing still renders after a full reload.
  await page.getByRole("button", { name: "Up one level" }).click();
  await page.reload();
  await page.getByRole("button", { name: "Browser Docs", exact: true }).waitFor();

  assert.deepEqual(consoleErrors, [], "console must stay clean");
  console.log("files browser verification passed");
} finally {
  if (browser) await browser.close();
  await stopDarWithOidc(context);
  await rm(dataRoot, { recursive: true, force: true });
}
