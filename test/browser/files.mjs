// Manual browser verification for the Files workspace. Launches an
// isolated Dar instance with the hermetic OIDC provider, drives a real
// headless Chromium through the Files page flows (New menu, URL deep
// links, editor with draft/If-Match save, trash restore, settings with
// revision-backed preferences, DAV app passwords), and fails on any
// console error. Run inside `nix develop` so the packaged Chromium is
// available:
//   node test/browser/files.mjs
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { Buffer } from "node:buffer";
import { execFileSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { chromium } from "../helpers/browser.mjs";
import { expect } from "@playwright/test";

import {
  startDarWithOidc,
  loginSession,
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

function draftCount(page) {
  return page.evaluate(
    () => Object.keys(localStorage).filter((key) => key.startsWith("dar.files.draft:")).length,
  );
}

const dataRoot = await mkdtemp("/tmp/dar-files-browser-");
const context = await startDarWithOidc(dataRoot, {
  DAR_CREDENTIAL_ENCRYPTION_KEYS: `fixture:${Buffer.alloc(32, 29).toString("base64")}`,
});
const consoleErrors = [];
let browser, page;
try {
  await waitForHealth(context.port, context.running.child, context.running.output);
  browser = await chromium.launch({
    executablePath: chromiumExecutable(),
    headless: true,
  });
  page = await browser.newPage();
  page.on("console", (message) => {
    const text = message.text();
    // The session probe legitimately returns 401 before sign-in; the
    // browser logs it as a resource error. Anything else must stay clean.
    const expectedFailure = (message.location().url.includes("/api/v1/files/comments") && /\b503\b/.test(text)) ||
      (message.location().url.includes("/api/v1/files/user-shares") && /\b404\b/.test(text)) ||
      (message.location().url.includes("/api/v1/files/editor") && /\b(412|503)\b/.test(text));
    if (message.type() === "error" && !text.includes("401 (Unauthorized)") && !expectedFailure) {
      consoleErrors.push(text);
    }
  });
  page.on("pageerror", (error) => consoleErrors.push(String(error)));
  const outstanding = new Set();

  await page.goto(context.baseUrl);
  await page.waitForTimeout(700);
  await page.getByRole("link", { name: "Continue with OpenID Connect" }).click();
  await page.getByRole("heading", { name: /Welcome,/ }).waitFor({ timeout: 15000 });
  const session = await loginSession(context.baseUrl, context.running.output);
  const deletePreferences = await fetch(context.baseUrl + "/api/v1/files/settings", {
    method: "PUT", headers: {...session.authHeaders, "if-match": '"0"'},
    body: JSON.stringify({settings: {warnOnFileDelete: true, warnOnFolderDelete: true}}),
  });
  assert.equal(deletePreferences.status, 200, await deletePreferences.clone().text());
  const get = async path => {
    const response = await fetch(context.baseUrl + path, { headers: session.authHeaders });
    assert.equal(response.status, 200, await response.clone().text()); return response.json();
  };

  // The Files page lists an empty folder with the New menu.
  await page.getByRole("link", { name: "Files" }).first().click();
  await page.getByRole("heading", { name: "Files", exact: true }).waitFor({ timeout: 15000 });
  await page.getByText("No files in here").waitFor();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Home").waitFor();

  // The Files toolbar has its own New menu; scope below the app shell.
  const stage = page.getByRole("main");

  // The New menu exposes creation, uploads, and file requests.
  await stage.getByRole("button", { name: "New", exact: true }).click();
  await page.getByRole("menuitem", { name: "New folder" }).waitFor();
  await page.getByRole("menuitem", { name: "Upload files" }).waitFor();
  await page.getByRole("menuitem", { name: "Create file request" }).waitFor();
  await page.keyboard.press("Escape");
  await page.getByRole("menuitem", { name: "New folder" }).waitFor({ state: "detached" });

  // Create a folder through the New menu and open it.
  await stage.getByRole("button", { name: "New", exact: true }).click();
  await page.getByRole("menuitem", { name: "New folder" }).click();
  await page.getByRole("dialog").getByRole("heading", { name: "New folder" }).waitFor();
  await page.getByLabel("Name").fill("Browser Docs");
  await page.getByRole("dialog").getByRole("button", { name: "Create" }).click();
  await page.getByRole("button", { name: "Browser Docs", exact: true }).click();
  await page.getByText("No files in here").waitFor();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Browser Docs").waitFor();

  // Create an empty text file and favorite it with the row star.
  await stage.getByRole("button", { name: "New", exact: true }).click();
  await page.getByRole("menuitem", { name: "New text file" }).click();
  await page.getByRole("dialog").getByRole("heading", { name: "New text file" }).waitFor();
  await page.getByLabel("Name").fill("welcome.md");
  await page.getByRole("dialog").getByRole("button", { name: "Create" }).click();
  // Extensions are hidden by default, so the visible name drops ".md".
  await page.getByRole("button", { name: "welcome.md", exact: true }).waitFor();
  await page.getByRole("button", { name: "Add welcome.md to favorites" }).click();
  await page.getByRole("button", { name: "Remove welcome.md from favorites" }).waitFor();

  // Upload replaces the empty file's content through the hidden input.
  await page.locator("#sanctuary-files-upload-input").setInputFiles({
    name: "welcome.md",
    mimeType: "text/markdown",
    buffer: Buffer.from("# Hello from the browser\n"),
  });
  await page.getByText("25 B").waitFor();

  // Details mutations must update the open pane without a route change.
  let failComments = true;
  await page.route(context.baseUrl + "/api/v1/files/comments?**", async route => {
    if (failComments && route.request().method() === "GET") {
      failComments = false;
      await route.fulfill({ status: 503, contentType: "application/json", body: JSON.stringify({error:{code:"unavailable",message:"Comments are temporarily unavailable."}}) });
    } else await route.continue();
  });
  await page.getByRole("button", { name: "Details for welcome.md" }).click();
  const details = page.getByRole("complementary", { name: "Details for welcome.md" });
  const resource = encodeURIComponent("/Browser Docs/welcome.md");
  await expect(details.getByRole("button", { name: "Lock", exact: true })).toBeVisible();
  await details.getByRole("button", { name: "Lock", exact: true }).click();
  await expect(details.getByRole("button", { name: "Unlock", exact: true })).toBeVisible();
  assert.equal((await get("/api/v1/files/lock?path=" + resource)).locked, true);
  await details.getByRole("button", { name: "Unlock", exact: true }).click();
  await expect(details.getByRole("button", { name: "Lock", exact: true })).toBeVisible();
  await details.getByRole("button", { name: "Edit tags", exact: true }).click();
  const tags = page.getByRole("dialog", { name: "Tags for welcome.md", exact: true });
  await tags.getByLabel("New tag", { exact: true }).fill("Reviewed");
  await tags.getByRole("button", { name: "Create tag", exact: true }).click();
  await expect(tags.getByRole("checkbox", { name: "Reviewed", exact: true })).toBeChecked();
  await tags.getByRole("button", { name: "Done", exact: true }).click();
  await expect(details.getByRole("button", { name: "Reviewed", exact: true })).toBeVisible();
  // Reopen to prove a facet failure exposes retry instead of an empty state.
  failComments = true;
  await details.getByRole("button", { name: "Close details" }).click();
  await page.getByRole("button", { name: "Details for welcome.md" }).click();
  await details.getByRole("tab", { name: "Comments", exact: true }).click();
  await page.keyboard.press("ArrowLeft");
  await expect(details.getByRole("tab", { name: "Versions", exact: true })).toBeFocused();
  await page.keyboard.press("ArrowRight");
  await expect(details.getByRole("tab", { name: "Comments", exact: true })).toBeFocused();
  await expect(details.getByRole("alert")).toHaveText(/temporarily unavailable/);
  await details.getByRole("button", { name: "Try again", exact: true }).click();
  await expect(details.getByText("No comments yet.", { exact: true })).toBeVisible();
  await details.getByLabel("Add a comment", { exact: true }).fill("Ready for review");
  await details.getByRole("button", { name: "Comment", exact: true }).click();
  await expect(details.getByText("Ready for review", { exact: true })).toBeVisible();
  await expect.poll(async () => (await get("/api/v1/files/comments?path=" + resource)).items.length).toBe(1);
  await page.reload();
  await expect(details.getByText("Ready for review", { exact: true })).toBeVisible();
  await details.getByRole("button", { name: "Delete comment", exact: true }).click();
  await expect(details.getByText("No comments yet.", { exact: true })).toBeVisible();
  await details.getByRole("tab", { name: "Sharing", exact: true }).click();
  await details.getByLabel("Account email", { exact: true }).fill("missing@example.test");
  await details.getByRole("button", { name: "Share", exact: true }).click();
  await expect(details.getByRole("alert")).toHaveText("Recipient not found.");
  await expect(details.getByLabel("Account email", { exact: true })).toHaveValue("missing@example.test");
  await expect(details.getByRole("button", { name: "Share", exact: true })).toBeEnabled();
  const identities = new DatabaseSync(dataRoot + "/dar.sqlite");
  identities.prepare("INSERT INTO users(id, subject, display_name, email, administrator, created_at, updated_at) VALUES (?, ?, ?, ?, 0, ?, ?)")
    .run("files-reader", "files-reader", "Reader", "reader@example.test", new Date().toISOString(), new Date().toISOString());
  identities.close();
  await details.getByLabel("Account email", { exact: true }).fill("reader@example.test");
  await details.getByLabel("Permission", { exact: true }).selectOption("write");
  await details.getByRole("button", { name: "Share", exact: true }).click();
  await expect(details.getByLabel("Account email", { exact: true })).toHaveValue("");
  await expect.poll(async () => (await get("/api/v1/files/user-shares")).items.length).toBe(1);
  const grant = (await get("/api/v1/files/user-shares")).items[0];
  assert.equal(grant.toUser,"files-reader"); assert.equal(grant.permission,"write");
  await page.reload();
  await expect(details.getByText("files-reader (write)", { exact: true })).toBeVisible();
  await details.getByRole("button", { name: "Revoke", exact: true }).click();
  await expect(details.getByText("Nobody else has access.", { exact: true })).toBeVisible();
  assert.equal((await get("/api/v1/files/user-shares")).items.length,0);
  await details.getByLabel("Password (optional)", { exact: true }).fill("local-fixture-password");
  await details.getByRole("button", { name: "Create public link", exact: true }).click();
  await expect(details.locator("code")).toHaveCount(1);
  const link = await details.locator("code").textContent();
  assert.match(link, /\/share\//);
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"], { origin: context.baseUrl });
  await details.getByRole("button", { name: "Copy link", exact: true }).click();
  await expect.poll(async () => page.evaluate(() => navigator.clipboard.readText())).toBe(link);
  await page.evaluate(() => Object.defineProperty(navigator.clipboard, "writeText", {
    configurable: true, value: () => Promise.reject(new DOMException("Fixture clipboard permission denied", "NotAllowedError")),
  }));
  await details.getByRole("button", { name: "Copy link", exact: true }).click();
  await expect(page.getByText(`Copy unavailable. Select the link: ${link}`, { exact: true })).toBeVisible();
  await page.evaluate(() => { delete navigator.clipboard.writeText; });
  assert.equal((await get("/api/v1/files/shares")).items.length, 1);
  await details.getByRole("button", { name: "Revoke", exact: true }).click();
  await expect(details.getByText("No links yet.", { exact: true })).toBeVisible();
  assert.equal((await get("/api/v1/files/shares")).items.length, 0);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: "/tmp/dar-files-details-mobile.png" });
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await details.getByRole("button", { name: "Close details" }).click();
  await details.waitFor({ state: "detached" });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.unroute(context.baseUrl + "/api/v1/files/comments?**");

  // Delete through the row menu and the product dialog.
  await page.getByRole("button", { name: "More actions for welcome.md" }).click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  await page.getByRole("dialog").getByRole("heading", { name: "Move to trash?" }).waitFor();
  await page.getByRole("button", { name: "Confirm delete" }).click();
  await page.getByText("No files in here").waitFor();

  // The folder listing still renders after a full reload.
  await page.getByRole("navigation", { name: "Folder path" })
    .getByRole("button", { name: "Home" }).click();
  await page.reload();
  await page.getByRole("button", { name: "Browser Docs", exact: true }).waitFor();

  // --- Trash restore -----------------------------------------------------
  // The trashed welcome.md returns to its folder via the Trash view.
  const sections = page.getByRole("navigation", { name: "Files sections" });
  await sections.getByRole("button", { name: "Trash" }).click();
  await page.getByRole("button", { name: "Restore", exact: true }).waitFor();
  await page.getByRole("button", { name: "Delete forever", exact: true }).waitFor();
  await page.getByRole("button", { name: "Restore", exact: true }).click();
  await page.getByText("The trash is empty").waitFor();
  await sections.getByRole("button", { name: "All files" }).click();
  await page.getByRole("button", { name: "Browser Docs", exact: true }).click();
  await page.getByRole("button", { name: "welcome.md", exact: true }).waitFor();

  // --- Deep-link reload restores the folder ------------------------------
  // The URL now carries the folder; a reload must land back inside it.
  assert.ok(page.url().includes("view=all"), "folder URL carries the view param");
  assert.ok(page.url().includes("path="), "folder URL carries the path param");
  await page.reload();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Browser Docs").waitFor();
  await page.getByRole("button", { name: "welcome.md", exact: true }).waitFor();

  // --- Rename flow through the row menu ----------------------------------
  await page.getByRole("button", { name: "More actions for welcome.md" }).click();
  await page.getByRole("menuitem", { name: "Rename" }).click();
  await page.getByRole("dialog").getByRole("heading", { name: "Rename" }).waitFor();
  await page.getByLabel("New name").fill("hello.md");
  await page.getByRole("dialog").getByRole("button", { name: "Rename", exact: true }).click();
  await page.getByRole("button", { name: "hello.md", exact: true }).waitFor();

  // --- Favorites view ----------------------------------------------------
  // The star survived trash, restore, and rename, so hello.md is still
  // favorited and the Favorites view must list it without re-starring.
  await page.getByRole("button", { name: "Remove hello.md from favorites" }).waitFor();
  await sections.getByRole("button", { name: "Favorites" }).click();
  await page.getByRole("button", { name: "hello.md", exact: true }).waitFor();
  await sections.getByRole("button", { name: "All files" }).click();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Browser Docs").waitFor();

  // --- Editor: rich pane, source mode, draft, save -----------------------
  await page.getByRole("button", { name: "hello.md", exact: true }).click();
  const editor = page.locator(".sanctuary-editor");
  await editor.getByRole("textbox", { name: "File name" }).waitFor();
  await editor.getByRole("button", { name: "Close editor", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  assert.equal(await page.evaluate(() => document.querySelector('.sanctuary-editor').contains(document.activeElement)), true);
  assert.ok(page.url().includes("open="), "editor URL carries the open param");
  // Markdown opens in the rich pane; switch to source for exact input.
  await editor.getByRole("button", { name: "Source" }).click();
  const source = editor.getByRole("textbox", { name: "Source text" });
  await source.fill("# Edited in browser\n");
  // The draft debounce lands in localStorage while editing.
  await page.waitForTimeout(1100);
  assert.equal(await draftCount(page), 1, "editing writes one debounced draft");
  await editor.getByRole("button", { name: "Save" }).click();
  await editor.getByText("Saved.").waitFor();
  assert.equal(await draftCount(page), 0, "saving clears the draft");
  await editor.getByRole("button", { name: "Close editor" }).click();
  await page.locator(".sanctuary-editor").waitFor({ state: "detached" });

  // Comparing and restoring a saved version keeps the replaced content too.
  await page.getByRole("button", { name: "Details for hello.md", exact: true }).click();
  const renamedDetails = page.getByRole("complementary", { name: "Details for hello.md", exact: true });
  await renamedDetails.getByRole("tab", { name: "Versions", exact: true }).click();
  await expect(renamedDetails.getByRole("checkbox")).toHaveCount(2);
  await renamedDetails.getByRole("checkbox").nth(0).click();
  await expect(renamedDetails.getByRole("checkbox").nth(0)).toBeChecked();
  await renamedDetails.getByRole("checkbox").nth(1).click();
  await expect(renamedDetails.getByRole("checkbox").nth(1)).toBeChecked();
  await renamedDetails.getByRole("button", { name: "Compare selected", exact: true }).click();
  const diff = page.getByRole("dialog", { name: "Version differences", exact: true });
  await expect(diff.locator("pre")).toHaveText(/Hello from the browser/);
  await page.keyboard.press("Escape");
  await expect(diff).toBeHidden();
  await expect(renamedDetails).toBeVisible();
  await expect(renamedDetails.getByRole("button", { name: "Compare selected", exact: true })).toBeFocused();
  await renamedDetails.getByRole("button", { name: "Restore", exact: true }).first().click();
  const editorPath = "/api/v1/files/editor?path=" + encodeURIComponent("/Browser Docs/hello.md");
  await expect.poll(async () => (await get(editorPath)).content).toBe("# Hello from the browser\n");
  await expect(renamedDetails.getByRole("checkbox")).toHaveCount(3);
  await renamedDetails.getByRole("button", { name: "Close details", exact: true }).click();
  await page.getByRole("button", { name: "hello.md", exact: true }).click();
  await editor.getByRole("button", { name: "Source", exact: true }).click();
  await source.fill("# Edited in browser\n");
  await editor.getByRole("button", { name: "Save", exact: true }).click();
  await editor.getByText("Saved.", { exact: true }).waitFor();
  await editor.getByRole("button", { name: "Close editor", exact: true }).click();

  // --- Editor deep link --------------------------------------------------
  // Opening /files with view, path, and open restores the whole
  // workspace, editor included, without touching history.
  await page.goto(
    context.baseUrl +
      "/files?view=all&path=%2FBrowser%20Docs&open=%2FBrowser%20Docs%2Fhello.md",
  );
  await editor.getByRole("textbox", { name: "File name" }).waitFor({ timeout: 15000 });
  await editor.getByRole("button", { name: "Source" }).click();
  const restored = editor.getByRole("textbox", { name: "Source text" });
  assert.equal(await restored.inputValue(), "# Edited in browser\n");
  const changeFromAnotherDevice = async content => {
    const current = await get(editorPath);
    const response = await fetch(context.baseUrl + editorPath, {
      method: "PUT", headers: {...session.authHeaders, "if-match": `"${current.etag}"`}, body: JSON.stringify({content}),
    });
    assert.equal(response.status, 200, await response.clone().text());
  };
  await restored.fill("# An unsaved browser draft\n");
  await changeFromAnotherDevice("# Saved on another device\n");
  let failSavedRead = true;
  await page.route(context.baseUrl + "/api/v1/files/editor?**", async route => {
    if (failSavedRead && route.request().method() === "GET") {
      failSavedRead = false;
      await route.fulfill({status:503,contentType:"application/json",body:JSON.stringify({error:{code:"unavailable",message:"Saved content is temporarily unavailable."}})});
    } else await route.continue();
  });
  await editor.getByRole("button", { name: "Save", exact: true }).click();
  await expect(editor.getByRole("alert")).toHaveText(/Could not load the saved version/);
  await expect(editor.getByRole("button", { name: "Use the saved version", exact: true })).toBeDisabled();
  await expect(editor.getByRole("button", { name: "Keep my changes", exact: true })).toBeDisabled();
  await editor.getByRole("button", { name: "Try again", exact: true }).click();
  await expect(editor.getByRole("button", { name: "Use the saved version", exact: true })).toBeEnabled();
  await page.unroute(context.baseUrl + "/api/v1/files/editor?**");
  await expect(editor.getByRole("heading", { name: "Your version", exact: true })).toBeVisible();
  await expect(editor.getByRole("heading", { name: "Saved version", exact: true })).toBeVisible();
  await expect(editor.getByRole("button", { name: "Save", exact: true })).toBeDisabled();
  await page.setViewportSize({width:390,height:844});
  await page.screenshot({path:"/tmp/dar-files-editor-conflict-mobile.png"});
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
  await editor.getByRole("button", { name: "Use the saved version", exact: true }).click();
  await expect(restored).toHaveValue("# Saved on another device\n");
  await restored.fill("# Edited after reviewing saved text\n");
  await editor.getByRole("button", { name: "Save", exact: true }).click();
  await expect.poll(async () => (await get(editorPath)).content).toBe("# Edited after reviewing saved text\n");
  await restored.fill("# Reviewed local changes\n");
  await changeFromAnotherDevice("# Another concurrent edit\n");
  await editor.getByRole("button", { name: "Save", exact: true }).click();
  await expect(editor.getByRole("button", { name: "Keep my changes", exact: true })).toBeVisible();
  await editor.getByRole("button", { name: "Keep my changes", exact: true }).click();
  await expect.poll(async () => (await get(editorPath)).content).toBe("# Reviewed local changes\n");
  await page.setViewportSize({width:1440,height:1000});
  await page.keyboard.press("Escape");
  await page.locator(".sanctuary-editor").waitFor({ state: "detached" });

  // --- Search with type filter -------------------------------------------
  // The search box lives in the stage toolbar; submitting navigates to
  // the search view where the type filter narrows the results.
  await page.getByRole("searchbox", { name: "Search files" }).fill("hello");
  await page.keyboard.press("Enter");
  await page.getByRole("combobox", { name: "Search file type" }).waitFor();
  await page.getByRole("button", { name: "hello.md", exact: true }).waitFor();
  await page.getByRole("combobox", { name: "Search file type" }).selectOption("folder");
  await page.getByText("No files in here").waitFor();
  await page.getByRole("combobox", { name: "Search file type" }).selectOption("document");
  await page.getByRole("button", { name: "hello.md", exact: true }).waitFor();


  // --- Shares and mail views stay available ------------------------------
  await sections.getByRole("button", { name: "Shared" }).click();
  await page.getByRole("heading", { name: "Shared with you" }).waitFor();
  await page.getByRole("heading", { name: "Shared by you" }).waitFor();
  await page.getByText("Nothing is shared with you yet.").waitFor();
  await sections.getByRole("button", { name: "Mail attachments" }).click();
  await page.getByText("No mail attachments").waitFor();

  // --- Settings: revision-backed workspace preferences -------------------
  // Switching the default view to grid re-renders the listing as cards;
  // the save round-trip carries the revision-backed If-Match header.
  await sections.getByRole("button", { name: "Settings" }).click();
  await page.getByText("Sorting and layout").waitFor();
  const layout = page.getByLabel("Default view");
  // A settings re-render can land between Playwright's value change and
  // the change event, silently reverting the selection; retry until the
  // controlled select keeps the new value.
  for (let attempt = 0; attempt < 6; attempt++) {
    await layout.selectOption("grid");
    try {
      await page.waitForFunction(
        () => { const sel = document.querySelector('select'); return sel && sel.value === 'grid'; },
        null, { timeout: 250 });
      break;
    } catch (err) { /* the re-render reverted the select; try again */ }
  }
  await page.getByRole("button", { name: "Save changes" }).click();
  await page.getByText("Settings saved.").waitFor();

  // The saved default applies to the listing immediately.
  await sections.getByRole("button", { name: "All files" }).click();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Browser Docs").waitFor();
  // Grid cards render without the list header row.
  await page.locator(".sanctuary-files-grid").waitFor();
  await page.getByRole("button", { name: "hello.md", exact: true }).waitFor();

  // File extensions preference: the card shows hello.md once enabled.
  await sections.getByRole("button", { name: "Settings" }).click();
  await page.getByText("Sorting and layout").waitFor();
  const extensions = page.locator("label.sanctuary-settings-check")
    .filter({ hasText: "Show file extensions" }).getByRole("checkbox");
  if (!(await extensions.isChecked())) {
    await extensions.click();
    await page.getByRole("button", { name: "Save changes" }).click();
    await page.getByText("Settings saved.").waitFor();
  }
  await sections.getByRole("button", { name: "All files" }).click();
  await page.getByRole("navigation", { name: "Folder path" })
    .getByText("Browser Docs").waitFor();
  await page.getByRole("button", { name: "hello.md", exact: true }).waitFor();

  // --- WebDAV endpoint and scoped app passwords --------------------------
  // The one-time app password is shown exactly once, then usable against
  // the DAV endpoint until revoked.
  await sections.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("heading", { name: "WebDAV" }).waitFor();
  await page.getByText(/\/dav\/files\/user-123\//).waitFor();
  await page.getByText("user-123", { exact: true }).waitFor();
  await page.getByLabel("App password label").fill("Browser DAV");
  await page.getByRole("button", { name: "Create Files app password" }).click();
  const reveal = page.locator(".sanctuary-password-reveal");
  await reveal.waitFor();
  const token = await reveal.locator("code").textContent();
  assert.ok(token && token.length > 20, "app password is revealed once");
  await page.getByText("Browser DAV", { exact: true }).waitFor();
  await page.getByText("Copy this app password now; it is never shown again.").waitFor();
  await page.getByRole("button", { name: "I saved it" }).click();

  const dav = await fetch(
    context.baseUrl + "/dav/files/user-123/",
    { method: "PROPFIND", headers: { Depth: "1", Authorization: "Basic " + Buffer.from("user-123:" + token).toString("base64") } },
  );
  assert.equal(dav.status, 207, "created app password authenticates DAV");

  await page.getByRole("button", { name: "Revoke", exact: true }).click();
  await page.getByText("No app passwords yet.").waitFor();
  const davAfter = await fetch(
    context.baseUrl + "/dav/files/user-123/",
    { method: "PROPFIND", headers: { Depth: "1", Authorization: "Basic " + Buffer.from("user-123:" + token).toString("base64") } },
  );
  assert.equal(davAfter.status, 401, "revoked app password is rejected");

  // The console stays fully clean: no React key diagnostics, no errors.
  assert.deepEqual(consoleErrors, [], "console must stay clean");
  console.log("files browser verification passed");
} catch (error) {
  if (page) {
    await page.screenshot({path:"/tmp/dar-files-browser-failure.png"});
    console.error(await page.locator("body").innerText());
    console.error("Console errors:", consoleErrors);
  }
  throw error;
} finally {
  if (browser) await browser.close();
  await stopDarWithOidc(context);
  await rm(dataRoot, { recursive: true, force: true });
}
