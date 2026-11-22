// Mail in a real browser on an isolated instance with a fake IMAP/SMTP provider: the three-pane
// layout, the reader with blocked remote images, inline reply with Undo, the compose sheet and
// drafts, archive with Undo by keyboard, settings with providers, adding an account, phones and
// both themes.
import assert from "node:assert/strict";
import {mkdtemp, readFile, writeFile} from "node:fs/promises";
import path from "node:path";
import {randomUUID} from "node:crypto";
import {chromium} from "@playwright/test";
import {freePort, publishModule, startInstance} from "./instance.ts";
import {message, startMailServer} from "./support/mail-server.mjs";

if (!process.env.DAR_RUST_RUNTIME) throw Error("Run through rust/verify.mjs.");
const root = await mkdtemp("/tmp/dar-mail-browser-"), name = "dar-mail-ui-" + randomUUID().slice(0, 8), port = await freePort(), base = `http://127.0.0.1:${port}`;
publishModule(name, base);
const server = await startMailServer(root), s = server.state;
s.mail.put("INBOX", message({subject: "Q4 venue shortlist", id: "v1@example.test", body: "Two venues so far.", date: new Date(Date.now() - 86400_000 * 3)}), ["\\Seen"]);
s.mail.put("INBOX", message({subject: "Re: Q4 venue shortlist", id: "v2@example.test", references: ["v1@example.test"], cc: "Rami <rami@example.test>", date: new Date(Date.now() - 3600_000),
  html: `<p>I've attached the revised quote from the second venue.</p><p><img src="https://tracker.example/p.gif" alt="pixel"></p>`, attachment: {name: "quote.pdf", type: "application/pdf", data: "%PDF-1.4"}}));
s.mail.put("INBOX", message({subject: "Dinner on Friday?", from: "Omar Saleh <omar@example.test>", body: "Around 8?", date: new Date(Date.now() - 600_000)}));
const providers = path.join(root, "providers.json");
await writeFile(providers, JSON.stringify([
  {id: "work-mail", name: "Work Mail", document: {protocol: "imap", auth: "password", domains: ["example.test"], imap: {host: "127.0.0.1", port: server.imapPort, security: "tls"}, smtp: {host: "127.0.0.1", port: server.smtpPort, security: "tls"}}},
  {id: "google", name: "Google", document: {protocol: "imap", auth: "oauth", domains: ["gmail.com"], imap: {host: "127.0.0.1", port: server.imapPort, security: "tls"}, smtp: {host: "127.0.0.1", port: server.smtpPort, security: "tls"},
    oauth: {authorizeUrl: `${server.https}/oauth/authorize`, tokenUrl: `${server.https}/oauth/token`, clientId: "dar-test-client", scopes: ["mail"]}}},
]));
const runtime = await startInstance({port, origin: base, root: path.join(root, "state"), databaseName: name, development: true, env: {DAR_MAIL_PROVIDERS_FILE: providers, SSL_CERT_FILE: server.caFile}});
const browser = await chromium.launch({executablePath: process.env.DAR_CHROMIUM, headless: true});
try {
  const setup = await fetch(base + "/api/v1/auth/setup", {method: "POST", headers: {origin: base, "content-type": "application/json"}, body: JSON.stringify({username: "admin", displayName: "Anas", password: "a long isolated test password", bootstrapToken: await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE, "utf8")})});
  assert.equal(setup.status, 200);
  const context = await browser.newContext({viewport: {width: 1440, height: 950}}), page = await context.newPage(), errors = [];
  page.on("pageerror", e => errors.push(e.message)); page.on("console", m => { if (m.type() === "error" && !/Failed to load resource/.test(m.text())) errors.push(m.text()); });
  page.setDefaultTimeout(15000);
  await page.goto(`${base}/mail`);
  await page.getByLabel("Username", {exact: true}).fill("admin"); await page.getByLabel("Password", {exact: true}).fill("a long isolated test password"); await page.getByRole("button", {name: "Sign in", exact: true}).click();
  await page.getByRole("link", {name: "Mail", exact: true}).first().waitFor();
  // No accounts yet: the empty state offers Add account; the pane picks the provider by domain.
  await page.getByRole("heading", {name: "Add your first mail account"}).waitFor();
  await page.getByRole("button", {name: "Add account", exact: true}).last().click();
  const pane = page.locator("[data-editor-pane]");
  await pane.getByLabel("Email address", {exact: true}).fill("someone@gmail.com");
  assert.equal(await pane.getByRole("radio", {name: /Google/}).isChecked(), true);
  await pane.getByRole("button", {name: "Continue with Google"}).waitFor();
  await pane.getByRole("radio", {name: /Work Mail/}).check();
  await pane.getByLabel("Email address", {exact: true}).fill("anas@example.test");
  await pane.getByLabel("Account name").fill("Work");
  await pane.getByLabel("Password or app password").fill("wrong password");
  await pane.getByRole("button", {name: "Sign in and add"}).click();
  await pane.getByRole("alert").getByText(/rejected the sign-in/).waitFor();
  await pane.getByLabel("Password or app password").fill("imap secret password");
  await pane.getByRole("button", {name: "Sign in and add"}).click();
  await pane.waitFor({state: "detached"});
  // The first pass mirrors the mailbox; the list follows.
  const list = page.getByRole("list", {name: "All inboxes messages"});
  await list.getByRole("button", {name: /Dinner on Friday\?/}).waitFor({timeout: 40000});
  await page.getByRole("button", {name: /^Inbox/}).first().waitFor();
  await page.screenshot({path: path.join(root, "mail-desktop.png")});
  // Reader: the thread, blocked remote images, Show images, attachments.
  await list.getByRole("button", {name: /Re: Q4 venue shortlist/}).click();
  const reader = page.getByRole("complementary", {name: "Message"});
  await reader.getByRole("heading", {name: "Re: Q4 venue shortlist"}).waitFor();
  await reader.getByText("I've attached the revised quote").waitFor();
  await reader.getByText("1 remote image blocked").waitFor();
  assert.equal(await reader.locator(".mail-body img").count(), 0);
  await reader.getByRole("button", {name: "Show images"}).click();
  await reader.locator(".mail-body img[src*='/api/v1/mail/image?url=']").waitFor({state: "attached"});
  await reader.getByRole("link", {name: "Download quote.pdf"}).waitFor();
  await reader.getByText("2 messages").waitFor();
  // Inline reply (Reply all: it went to two people), Undo puts it in the compose sheet.
  await reader.getByRole("button", {name: "Reply all"}).click();
  const replyBox = reader.getByRole("textbox", {name: "Reply"});
  await replyBox.fill("Great, please confirm.");
  await reader.getByText(/to lina@example\.test, rami@example\.test/).waitFor();
  await reader.getByRole("button", {name: "Send", exact: true}).click();
  const toast = page.locator(".mail-toast");
  await toast.getByText(/Sending/).waitFor();
  await toast.getByRole("button", {name: "Undo"}).click();
  const sheet = page.getByRole("dialog", {name: "Compose message"});
  await sheet.waitFor();
  assert.match(await sheet.getByRole("textbox", {name: "Message"}).inputValue(), /^Great, please confirm\.[\s\S]*> I've attached/);
  await sheet.getByRole("textbox", {name: "Message"}).press("Control+Enter");
  await page.getByText("Message sent").waitFor({timeout: 20000});
  assert.equal(s.sent.length, 1, "Undo stopped the first send");
  assert.deepEqual(s.sent[0].to.sort(), ["lina@example.test", "rami@example.test"]);
  // Archive by keyboard, then Undo.
  await list.getByRole("button", {name: /Dinner on Friday\?/}).click();
  await reader.getByRole("heading", {name: "Dinner on Friday?"}).waitFor();
  await page.locator("body").press("e");
  await toast.getByText("Message archived").waitFor();
  assert.equal(await list.getByRole("button", {name: /Dinner on Friday\?/}).count(), 0);
  await toast.getByRole("button", {name: "Undo"}).click();
  await list.getByRole("button", {name: /Dinner on Friday\?/}).waitFor();
  // Compose a new message; closing saves a draft on the server.
  await page.getByRole("button", {name: "Compose", exact: true}).click();
  await sheet.waitFor();
  await sheet.getByLabel("To").fill("maya@example.test, not-an-address");
  await sheet.getByLabel("To").press("Enter");
  await sheet.getByText("not-an-address").waitFor();
  await sheet.getByRole("button", {name: "Remove not-an-address"}).click();
  await sheet.getByLabel("Subject").fill("Photos from the weekend");
  await sheet.getByRole("textbox", {name: "Message"}).fill("Here they are.");
  await sheet.getByRole("button", {name: "Close (saves a draft)"}).click();
  await page.getByText("Draft saved").first().waitFor();
  await page.getByRole("button", {name: /^Drafts/}).first().click();
  await page.getByRole("list", {name: "Drafts messages"}).getByRole("button", {name: /Photos from the weekend/}).click();
  await reader.getByRole("button", {name: "Continue editing"}).click();
  assert.equal(await sheet.getByLabel("Subject").inputValue(), "Photos from the weekend");
  await sheet.getByRole("button", {name: "Discard"}).click();
  await sheet.waitFor({state: "detached"});
  assert.equal(s.mail.boxes.get("Drafts").messages.length, 1);
  // Markdown: the toolbar formats the selection, Preview shows it, and it is sent as HTML.
  await page.getByRole("button", {name: "Compose", exact: true}).click();
  await sheet.waitFor();
  await sheet.getByLabel("To").fill("lina@example.test"); await sheet.getByLabel("To").press("Enter");
  await sheet.getByLabel("Subject").fill("Agenda");
  // No format choice: plain until a formatting button is used.
  assert.equal(await sheet.getByLabel("Message format").count(), 0);
  const body = sheet.getByRole("textbox", {name: "Message"});
  assert.equal(await sheet.getByRole("radio", {name: "Preview"}).count(), 0);
  await body.fill("Venue first");
  await body.evaluate(el => el.setSelectionRange(0, 5));
  await sheet.getByRole("button", {name: "Bold"}).click();
  assert.equal(await body.inputValue(), "**Venue** first");
  await sheet.getByRole("radio", {name: "Preview"}).check();
  await sheet.locator(".mail-compose-preview strong", {hasText: "Venue"}).waitFor();
  await sheet.getByRole("button", {name: "Send", exact: true}).click();
  await page.getByText("Message sent").waitFor({timeout: 20000});
  assert.match(s.sent.at(-1).data, /<strong>Venue<\/strong> first/);
  assert.match(s.sent.at(-1).data, /\*\*Venue\*\* first/);
  // Settings: accounts, reading, and providers for the administrator.
  await page.getByRole("button", {name: "Mail settings"}).first().click();
  const settings = page.getByRole("dialog", {name: "Mail settings"});
  await settings.getByText("anas@example.test · Work Mail · IMAP and SMTP").waitFor();
  await settings.getByRole("tab", {name: "Reading"}).click();
  await settings.getByRole("radio", {name: "Load"}).check(); await settings.getByText("Saved").first().waitFor();
  await settings.getByRole("radio", {name: "Block"}).check();
  await settings.getByRole("tab", {name: "Providers"}).click();
  await settings.getByText("From the host configuration").first().waitFor();
  await settings.getByRole("button", {name: "Add provider"}).click();
  await settings.getByLabel("Start from").selectOption("google");
  const nameField = settings.getByRole("textbox", {name: /^Name/});
  assert.equal(await nameField.inputValue(), "Google");
  await settings.getByText("/api/v1/mail/oauth/callback").waitFor();
  await settings.getByRole("button", {name: "Cancel"}).click();
  await settings.getByRole("button", {name: "Close settings"}).click();
  await settings.waitFor({state: "detached"});
  // Phones and both themes: no sideways scrolling; the message opens full screen with Back.
  await page.getByRole("button", {name: "All inboxes"}).first().click();
  for (const theme of ["latte", "mocha"]) {
    await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
    for (const width of [1440, 390]) {
      await page.setViewportSize({width, height: 844});
      await page.waitForTimeout(300);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth ? [...document.querySelectorAll("body *")].filter(e => e.getBoundingClientRect().right > innerWidth + 1 && !e.closest(".sidebar")).slice(0, 8).map(e => e.className) : []);
      assert.deepEqual(overflow, [], `${theme} ${width} overflow`);
      if (width === 390) {
        await list.getByRole("button", {name: /Dinner on Friday\?/}).click();
        const back = page.getByRole("button", {name: "Back to messages"});
        await back.waitFor();
        assert.equal(await page.locator(".mail-main").isVisible(), false);
        await page.screenshot({path: path.join(root, `mail-phone-${theme}.png`)});
        await back.click();
        await page.locator(".mail-main").waitFor();
      }
    }
  }
  assert.deepEqual(errors, []);
  console.log(`PASS Mail browser: add account with provider by domain and sign-in errors, mirrored list, reader with blocked and shown images, inline Reply all with Undo into the compose sheet, keyboard archive with Undo, drafts, settings with providers, phones and both themes. Screenshots in ${root}.`);
} finally {
  await browser.close();
  await runtime.close();
  await server.close();
}
