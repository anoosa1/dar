import assert from "node:assert/strict";
import {test} from "node:test";
import {mkdtemp, readFile, writeFile} from "node:fs/promises";
import path from "node:path";
import https from "node:https";
import {randomUUID} from "node:crypto";
import {freePort, publishModule, startInstance} from "./instance.ts";
import {login} from "./integration-helpers.ts";
// @ts-ignore: a plain ES module fixture
import {message, startMailServer} from "./support/mail-server.mjs";

type Mailbox = {id: string; name: string; role: string; parentId: string; total: number; unread: number; folderId: string};
type Account = {id: string; name: string; address: string; status: string; lastError: string; folderId: string; mailboxes: Mailbox[]; protocol: string; auth: string};
type Summary = {id: string; subject: string; fromName: string; fromAddress: string; seen: boolean; flagged: boolean; answered: boolean; draft: boolean; mailboxId: string; fileId: string; threadCount: number; attachments: boolean};

test("Mail: providers, IMAP, OAuth and JMAP accounts, the two-way Files mirror, sending and isolation", {skip: !process.env.DAR_INTEGRATION_ORIGIN, timeout: 300_000}, async () => {
  const root = await mkdtemp("/tmp/dar-mail-acceptance-"), name = "dar-mail-" + randomUUID().slice(0, 8), port = await freePort(), base = `http://127.0.0.1:${port}`;
  publishModule(name, base);
  const server = await startMailServer(root);
  const s = server.state;
  // Server mail before the account exists: a thread of two, an HTML message with a remote and an inline image, an attachment.
  s.mail.put("INBOX", message({subject: "Q4 venue shortlist", id: "venue-1@example.test", body: "Two venues so far."}), ["\\Seen"]);
  s.mail.put("INBOX", message({subject: "Re: Q4 venue shortlist", id: "venue-2@example.test", references: ["venue-1@example.test"], date: new Date("2026-11-21T09:00:00Z"),
    html: `<p onclick="x()">Revised quote attached <img src="https://tracker.example/p.gif"></p><script>bad()</script>`, attachment: {name: "quote.pdf", type: "application/pdf", data: "%PDF-1.4 quote"}}));
  s.mail.put("INBOX", message({subject: "Dinner on Friday?", from: "Omar Saleh <omar@example.test>", body: "Around 8?", date: new Date("2026-11-22T09:58:00Z")}));
  s.mail.put("Archive", message({subject: "Old receipt", from: "Shop <shop@example.test>"}), ["\\Seen"]);
  s.mail.add("Projects/Offsite");
  s.mail.put("Projects/Offsite", message({subject: "Bus times", from: "Rami <rami@example.test>"}), ["\\Seen"]);
  const secretFile = path.join(root, "oauth-secret"), providersFile = path.join(root, "mail-providers.json");
  await writeFile(secretFile, s.oauth.secret + "\n");
  const host = "127.0.0.1";
  await writeFile(providersFile, JSON.stringify([
    {id: "test-imap", name: "Test IMAP", document: {protocol: "imap", auth: "password", domains: ["example.test"], saveSent: true,
      imap: {host, port: server.imapPort, security: "tls"}, smtp: {host, port: server.smtpPort, security: "tls"}}},
    {id: "test-oauth", name: "Test OAuth", secretFile, document: {protocol: "imap", auth: "oauth", domains: ["oauth.example.test"], saveSent: false,
      imap: {host, port: server.imapPort, security: "tls"}, smtp: {host, port: server.smtpPort, security: "tls"},
      oauth: {authorizeUrl: `${server.https}/oauth/authorize`, tokenUrl: `${server.https}/oauth/token`, clientId: "dar-test-client", scopes: ["mail"], params: {access_type: "offline"}}}},
    {id: "test-jmap", name: "Test JMAP", document: {protocol: "jmap", auth: "token", domains: [], jmap: {url: `${server.https}/jmap/session`}}},
  ]));
  const runtime = await startInstance({port, origin: base, root: path.join(root, "state"), databaseName: name, development: true, env: {DAR_MAIL_PROVIDERS_FILE: providersFile, SSL_CERT_FILE: server.caFile}});
  try {
    const setup = await fetch(base + "/api/v1/auth/setup", {method: "POST", headers: {origin: base, "content-type": "application/json"}, body: JSON.stringify({username: "admin", displayName: "Anas", password: "a long isolated test password", bootstrapToken: await readFile(process.env.DAR_BOOTSTRAP_TOKEN_FILE!, "utf8")})});
    assert.equal(setup.status, 200);
    const session = await login(base), headers = {...session.headers, "content-type": "application/json"};
    const raw = (route: string, init: RequestInit = {}) => fetch(base + "/api/v1/mail" + route, {...init, headers: {...headers, ...init.headers}});
    const call = async <T>(route: string, body?: unknown, method = body === undefined ? "GET" : "POST"): Promise<T> => {
      const response = await raw(route, {method, ...(body !== undefined ? {body: JSON.stringify(body)} : {})});
      assert.ok(response.ok, `${method} ${route}: ${response.status} ${await response.clone().text()}`);
      return response.json() as Promise<T>;
    };
    const overview = () => call<{providers: {id: string; auth: string}[]; accounts: Account[]; counts: {inbox: number; unread: number; flagged: number; drafts: number}; mailFolder: string; administrator: boolean}>("/overview");
    const list = async (view: string, id?: string) => (await call<{items: Summary[]}>(`/messages?view=${view}${id ? `&id=${id}` : ""}`)).items;
    const sync = (id: string) => call("/sync", {id});
    const mailbox = async (account: string, role: string, nameIs?: string) => (await overview()).accounts.find(a => a.id === account)!.mailboxes.find(m => nameIs ? m.name === nameIs : m.role === role)!;

    // Providers from the host configuration; Administration adds and removes its own.
    let o = await overview();
    assert.deepEqual(o.providers.map(p => p.id), ["test-imap", "test-oauth", "test-jmap"]);
    assert.equal(o.mailFolder, "docs/mail"); assert.equal(o.administrator, true);
    const admin = await call<{items: {id: string; source: string; hasSecret: boolean}[]}>("/admin/providers");
    assert.deepEqual(admin.items.map(p => [p.id, p.source]), [["test-imap", "nix"], ["test-oauth", "nix"], ["test-jmap", "nix"]]);
    assert.ok(!JSON.stringify(admin).includes(s.oauth.secret), "secrets never leave the server");
    assert.equal((await raw("/admin/providers/test-imap", {method: "PUT", body: JSON.stringify({name: "Changed", document: {protocol: "jmap", auth: "token", jmap: {url: "https://x.test/"}}})})).status, 403);
    const lan = await call<{id: string; hasSecret: boolean}>("/admin/providers/lan", {name: "LAN", create: true, secret: "lan-secret", document: {protocol: "imap", auth: "password", domains: ["lan.test"],
      imap: {host: "mail.lan.test", port: 993, security: "tls"}, smtp: {host: "mail.lan.test", port: 587, security: "starttls"}}}, "PUT");
    assert.equal(lan.hasSecret, true);
    assert.equal((await raw("/admin/providers/bad", {method: "PUT", body: JSON.stringify({name: "Bad", document: {protocol: "imap", auth: "password", imap: {host: "x", port: 99999, security: "tls"}}})})).status, 400);
    assert.ok((await overview()).providers.some(p => p.id === "lan"));
    await call("/admin/providers/lan", undefined, "DELETE");

    // An IMAP account: a wrong password is refused before anything is saved.
    const wrong = await raw("/accounts", {method: "POST", body: JSON.stringify({providerId: "test-imap", name: "Work", address: "anas@example.test", password: "nope"})});
    assert.equal(wrong.status, 502); assert.match((await wrong.json()).error.message, /rejected the sign-in/);
    assert.equal((await overview()).accounts.length, 0);
    const work = await call<Account>("/accounts", {providerId: "test-imap", name: "Work", address: "anas@example.test", displayName: "Anas", password: "imap secret password"});
    await sync(work.id);
    o = await overview();
    const account = o.accounts[0];
    assert.equal(account.status, "ready", account.lastError);
    assert.deepEqual(account.mailboxes.map(m => [m.name, m.role]).sort(), [["Archive", "archive"], ["Drafts", "drafts"], ["Inbox", "inbox"], ["Junk", "junk"], ["Offsite", ""], ["Projects", ""], ["Sent", "sent"], ["Trash", "trash"]]);
    const offsite = account.mailboxes.find(m => m.name === "Offsite")!, projects = account.mailboxes.find(m => m.name === "Projects")!;
    assert.equal(offsite.parentId, projects.id);
    assert.equal(o.counts.inbox, 2);
    let inbox = await list("inbox");
    assert.deepEqual(inbox.map(m => m.subject), ["Dinner on Friday?", "Re: Q4 venue shortlist", "Q4 venue shortlist"]);
    const reply = inbox[1];
    assert.equal(reply.threadCount, 2); assert.equal(reply.attachments, true); assert.equal(reply.seen, false);
    assert.deepEqual((await list("mailbox", offsite.id)).map(m => m.subject), ["Bus times"]);

    // Reading comes from the .eml file: sanitized, remote images blocked until asked, inline parts served.
    const detail = await call<{body: {html: string; blockedImages: number; attachments: {index: number; name: string}[]}; thread: Summary[]; fileName: string; mailboxRole: string}>(`/messages/${reply.id}`);
    assert.equal(detail.body.blockedImages, 1);
    assert.ok(!/script|onclick|tracker\.example/.test(detail.body.html), detail.body.html);
    assert.deepEqual(detail.thread.map(m => m.subject), ["Q4 venue shortlist", "Re: Q4 venue shortlist"]);
    assert.equal(detail.fileName, "2026-11-21 0900 Re: Q4 venue shortlist.eml");
    assert.match((await call<{body: {html: string}}>(`/messages/${reply.id}?images=1`)).body.html, /\/api\/v1\/mail\/image\?url=https%3A%2F%2Ftracker\.example%2Fp\.gif/);
    const pdf = await raw(`/messages/${reply.id}/attachments/0`);
    assert.equal(pdf.status, 200); assert.equal(await pdf.text(), "%PDF-1.4 quote"); assert.match(pdf.headers.get("content-disposition")!, /^attachment; filename="quote.pdf"/);
    assert.equal(pdf.headers.get("content-type"), "application/octet-stream");
    assert.equal((await raw(`/messages/${reply.id}/raw`)).status, 200);

    // The mirror in Files, through WebDAV.
    const credential = await (await fetch(base + "/api/v1/auth/app-passwords", {method: "POST", headers, body: JSON.stringify({label: "Mail mirror", scope: "v2 files:read files:write"})})).json() as {secret: string};
    const dav = {authorization: "Basic " + Buffer.from("admin:" + credential.secret).toString("base64")};
    const davRoot = `${base}/dav/files/admin/docs/mail/Work`;
    const listing = async (folder: string) => {
      const response = await fetch(`${davRoot}/${folder}/`, {method: "PROPFIND", headers: {...dav, depth: "1"}});
      assert.equal(response.status, 207, folder);
      return [...(await response.text()).matchAll(/<d:href>([^<]+)<\/d:href>/g)].map(m => decodeURIComponent(m[1]).split("/").filter(Boolean).at(-1)!).slice(1).sort();
    };
    assert.deepEqual(await listing("Inbox"), ["2026-11-20 1024 Q4 venue shortlist.eml", "2026-11-21 0900 Re: Q4 venue shortlist.eml", "2026-11-22 0958 Dinner on Friday?.eml"]);
    assert.deepEqual(await listing("Projects"), ["Offsite"]);
    const dinnerFile = `${davRoot}/Inbox/${encodeURIComponent("2026-11-22 0958 Dinner on Friday?.eml")}`;
    assert.match(await (await fetch(dinnerFile, {headers: dav})).text(), /Subject: Dinner on Friday\?/);

    // Flags and moves made in Mail reach the server; the file follows the message.
    await call("/messages/flags", {ids: [reply.id], seen: true, flagged: true});
    assert.ok(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Message-ID: <venue-2@"))!.message.flags.has("\\Flagged"));
    assert.equal((await overview()).counts.flagged, 1);
    const dinner = inbox[0];
    assert.deepEqual(await call("/messages/move", {ids: [dinner.id], role: "archive"}), {moved: 1});
    assert.equal(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Dinner on Friday"))!.box.name, "Archive");
    assert.ok((await listing("Archive")).includes("2026-11-22 0958 Dinner on Friday?.eml"));
    assert.ok(!(await listing("Inbox")).includes("2026-11-22 0958 Dinner on Friday?.eml"));
    // Undo moves it back by mailbox.
    await call("/messages/move", {ids: [dinner.id], mailboxId: (await mailbox(work.id, "inbox")).id});
    assert.equal(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Dinner on Friday"))!.box.name, "INBOX");

    // Two-way: changes made in Files go to the server on the next pass.
    const moveFile = await fetch(dinnerFile, {method: "MOVE", headers: {...dav, destination: `${davRoot}/Projects/Offsite/${encodeURIComponent("2026-11-22 0958 Dinner on Friday?.eml")}`}});
    assert.equal(moveFile.status, 201, await moveFile.clone().text());
    const venueFile = `${davRoot}/Inbox/${encodeURIComponent("2026-11-20 1024 Q4 venue shortlist.eml")}`;
    assert.equal((await fetch(venueFile, {method: "DELETE", headers: dav})).status, 204);
    const added = message({subject: "Added from Files", from: "Me <anas@example.test>", id: "from-files@example.test"});
    assert.equal((await fetch(`${davRoot}/Inbox/added.eml`, {method: "PUT", headers: dav, body: added})).status, 201);
    assert.equal((await fetch(`${davRoot}/Receipts/`, {method: "MKCOL", headers: dav})).status, 201);
    assert.equal((await fetch(`${davRoot}/Receipts/receipt.eml`, {method: "PUT", headers: dav, body: message({subject: "Kept receipt", id: "receipt@example.test"})})).status, 201);
    await sync(work.id);
    assert.equal(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Dinner on Friday"))!.box.name, "Projects/Offsite", "moved in Files → moved on the server");
    assert.equal(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Message-ID: <venue-1@"))!.box.name, "Trash", "deleted in Files → in Trash on the server");
    assert.equal(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Message-ID: <from-files@"))!.box.name, "INBOX", "added in Files → appended");
    assert.ok(s.created.includes("Receipts"), "a folder made in Files → a new mailbox");
    assert.equal(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Message-ID: <receipt@"))!.box.name, "Receipts");
    assert.deepEqual((await list("inbox")).map(m => m.subject).sort(), ["Added from Files", "Re: Q4 venue shortlist"]);
    // The added file is the message's file (no second copy appears on the next pass).
    await sync(work.id);
    assert.deepEqual((await listing("Inbox")).filter(n => n.endsWith(".eml")).length, 2);
    assert.deepEqual((await list("mailbox", (await mailbox(work.id, "", "Receipts")).id)).map(m => m.subject), ["Kept receipt"]);
    // Deleting a whole mailbox folder does not delete its messages: it is made again.
    assert.equal((await fetch(`${davRoot}/Archive/`, {method: "DELETE", headers: dav})).status, 204);
    await sync(work.id);
    assert.equal(s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Old receipt"))!.box.name, "Archive");
    assert.deepEqual(await listing("Archive"), ["2026-11-20 1024 Old receipt.eml"]);

    // Server → Files: new mail, removed mail and flag changes.
    const fresh = s.mail.put("INBOX", message({subject: "Fresh arrival", from: "Maya <maya@example.test>", date: new Date("2026-11-23T08:00:00Z")}));
    const replyOnServer = s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Message-ID: <venue-2@"))!;
    replyOnServer.message.flags.delete("\\Flagged");
    const added2 = s.mail.find((m: {data: Buffer}) => s.mail.text(m).includes("Message-ID: <from-files@"))!;
    added2.box.messages.splice(added2.box.messages.indexOf(added2.message), 1);
    await sync(work.id);
    inbox = await list("inbox");
    assert.deepEqual(inbox.map(m => m.subject), ["Fresh arrival", "Re: Q4 venue shortlist"]);
    assert.equal(inbox[1].flagged, false);
    assert.ok(!(await listing("Inbox")).includes("added.eml"), "removed on the server → its file moved to deleted files");
    void fresh;

    // Sending: SMTP with Bcc only in the envelope, a copy in Sent, the parent answered.
    const sent = await call<{id: string; messageId: string}>("/send", {accountId: work.id, to: ["lina@example.test"], cc: [], bcc: ["boss@example.test"], subject: "Re: Q4 venue shortlist", text: "Go ahead.\n.Leading dot", inReplyTo: reply.id,
      attachments: [{name: "notes.txt", type: "text/plain", data: Buffer.from("plan").toString("base64")}]});
    assert.equal(s.sent.length, 1);
    assert.deepEqual(s.sent[0].to, ["lina@example.test", "boss@example.test"]);
    assert.equal(s.sent[0].from, "anas@example.test");
    assert.ok(!/^Bcc:/mi.test(s.sent[0].data) && /In-Reply-To: <venue-2@example\.test>/.test(s.sent[0].data) && /References: <venue-1@example\.test> <venue-2@example\.test>/.test(s.sent[0].data), s.sent[0].data);
    assert.match(s.sent[0].data, /\r\n\.Leading dot/);
    assert.equal(s.appended.at(-1)!.box, "Sent");
    assert.ok(replyOnServer.message.flags.has("\\Answered"));
    assert.ok(sent.id);
    assert.deepEqual((await list("sent")).map(m => m.subject), ["Re: Q4 venue shortlist"]);
    assert.equal((await call<{thread: Summary[]}>(`/messages/${reply.id}`)).thread.length, 3, "the reply joins the thread");
    await sync(work.id);
    assert.equal((await list("sent")).length, 1, "the Sent copy is not mirrored twice");
    // Drafts: saved on the server; saving again replaces it.
    const draft = await call<{id: string}>("/drafts", {accountId: work.id, to: [], subject: "Unfinished", text: "first"});
    const draft2 = await call<{id: string}>("/drafts", {accountId: work.id, to: ["x@example.test"], subject: "Unfinished", text: "second", replaces: draft.id});
    assert.equal((await overview()).counts.drafts, 1);
    assert.deepEqual((await list("drafts")).map(m => [m.id, m.draft]), [[draft2.id, true]]);
    assert.equal(s.mail.boxes.get("Drafts").messages.length, 1);
    assert.ok(s.mail.boxes.get("Drafts").messages[0].flags.has("\\Draft"));
    // Markdown: sent with an HTML part made by Dar; the text part keeps the source.
    await call("/send", {accountId: work.id, to: ["lina@example.test"], subject: "Plan", text: "Hello **team**\n\n- one\n- [two](https://x.test)\n\n<script>x()</script>", format: "markdown"});
    const md = s.sent.at(-1).data;
    assert.match(md, /multipart\/alternative/); assert.match(md, /X-Dar-Format: markdown/i);
    assert.match(md, /<strong>team<\/strong>/); assert.match(md, /<li><a href="https:\/\/x\.test">two<\/a><\/li>/);
    assert.match(md, /&lt;script&gt;/); assert.ok(!md.includes("<script>x()") || md.indexOf("<script>x()") < md.indexOf("text/html"), "raw HTML stays text in the HTML part");
    const mdDraft = await call<{id: string}>("/drafts", {accountId: work.id, to: [], subject: "Notes", text: "# Agenda\n\n*one*", format: "markdown"});
    const mdBody = (await call<{body: {format: string; textPart: string; html: string}}>(`/messages/${mdDraft.id}`)).body;
    assert.equal(mdBody.format, "markdown"); assert.equal(mdBody.textPart.trim(), "# Agenda\n\n*one*"); assert.match(mdBody.html, /<h1>Agenda<\/h1>/);
    assert.equal((await raw("/send", {method: "POST", body: JSON.stringify({accountId: work.id, to: ["a@example.test"], subject: "x", text: "y", format: "rtf"})})).status, 400);
    const badSend = await raw("/send", {method: "POST", body: JSON.stringify({accountId: work.id, to: ["not an address"], subject: "x", text: "y"})});
    assert.equal(badSend.status, 400);
    // Delete from Trash removes for good.
    const trash = await mailbox(work.id, "trash");
    const trashed = (await list("mailbox", trash.id))[0];
    await call("/messages/move", {ids: [trashed.id], role: "trash"});
    assert.equal(s.mail.boxes.get("Trash").messages.length, 0);
    // Search finds mail by subject and sender.
    const search = await (await fetch(`${base}/api/v1/search?q=fresh`, {headers})).json() as {groups?: {title: string; items: {title: string}[]}[]};
    assert.match(JSON.stringify(search), /Fresh arrival/);

    // OAuth: Dar sends the browser to the provider; the code comes back without the session cookie.
    s.oauth.expiresIn = 1;
    const start = await call<{url: string}>("/oauth/start", {providerId: "test-oauth", name: "OAuth", address: "oauth@example.test", displayName: "O"});
    assert.match(start.url, /code_challenge_method=S256/); assert.match(start.url, /access_type=offline/);
    // The browser's visit to the provider (trusting the fixture's CA).
    const ca = await readFile(server.caFile);
    const callback = await new Promise<string>((resolve, reject) => https.get(start.url, {ca}, res => { res.resume(); resolve(res.headers.location!); }).on("error", reject));
    assert.ok(callback.startsWith(base + "/api/v1/mail/oauth/callback?"));
    const back = await fetch(callback, {redirect: "manual"});
    assert.equal(back.status, 303); assert.match(back.headers.get("location")!, /^\/mail\?account=/);
    assert.equal((await fetch(callback, {redirect: "manual"})).headers.get("location"), "/mail?oauthError=expired", "a state works once");
    const oauthAccount = (await overview()).accounts.find(a => a.address === "oauth@example.test")!;
    await sync(oauthAccount.id);
    assert.equal((await overview()).accounts.find(a => a.id === oauthAccount.id)!.status, "ready");
    assert.ok(s.oauth.tokenRequests.some((r: Record<string, string>) => r.grant_type === "authorization_code" && r.client_secret === s.oauth.secret));
    assert.ok(s.oauth.tokenRequests.some((r: Record<string, string>) => r.grant_type === "refresh_token"), "an expired access token is refreshed");
    await call("/send", {accountId: oauthAccount.id, to: ["lina@example.test"], subject: "Via OAuth", text: "hi"});
    assert.equal(s.sent.at(-1).user, "oauth@example.test");

    // JMAP with an API token.
    s.jmap.add("mi", message({subject: "JMAP hello", from: "Fastmail <hi@example.test>"}));
    s.jmap.add("ma", message({subject: "JMAP archived"}), ["$seen"]);
    const badToken = await raw("/accounts", {method: "POST", body: JSON.stringify({providerId: "test-jmap", name: "Personal", address: "jmap@example.test", token: "wrong"})});
    assert.equal(badToken.status, 502);
    const jmapAccount = await call<Account>("/accounts", {providerId: "test-jmap", name: "Personal", address: "jmap@example.test", token: "jmap-test-token"});
    await sync(jmapAccount.id);
    const jmapInbox = await mailbox(jmapAccount.id, "inbox");
    const jmapMessages = await list("mailbox", jmapInbox.id);
    assert.deepEqual(jmapMessages.map(m => m.subject), ["JMAP hello"]);
    await call("/messages/flags", {ids: [jmapMessages[0].id], seen: true});
    assert.ok([...s.jmap.emails.values()].find((e: {blobId: string}) => s.jmap.blobs.get(e.blobId).toString().includes("JMAP hello")).keywords.has("$seen"));
    await call("/messages/move", {ids: [jmapMessages[0].id], role: "archive"});
    assert.ok([...s.jmap.emails.values()].find((e: {blobId: string}) => s.jmap.blobs.get(e.blobId).toString().includes("JMAP hello")).mailboxIds.ma);
    await call("/send", {accountId: jmapAccount.id, to: ["lina@example.test"], subject: "From JMAP", text: "hello"});
    assert.equal(s.jmap.submissions.length, 1); assert.match(s.jmap.submissions[0].data, /Subject: From JMAP/);
    assert.deepEqual(s.jmap.submissions[0].envelope.rcptTo, [{email: "lina@example.test"}]);
    assert.ok(s.jmap.calls.includes("EmailSubmission/set"));
    assert.deepEqual((await list("inbox")).map(m => m.subject).includes("JMAP hello"), false);

    // Isolation: another account sees none of this and cannot manage providers.
    assert.equal((await fetch(base + "/api/v1/auth/users", {method: "POST", headers, body: JSON.stringify({username: "other", displayName: "Other", password: "another long isolated password"})})).status, 201);
    const other = await login(base, "other", "another long isolated password"), otherHeaders = {...other.headers, "content-type": "application/json"};
    const otherOverview = await (await fetch(base + "/api/v1/mail/overview", {headers: otherHeaders})).json() as {accounts: unknown[]; administrator: boolean};
    assert.deepEqual(otherOverview.accounts, []); assert.equal(otherOverview.administrator, false);
    assert.equal((await fetch(`${base}/api/v1/mail/messages/${reply.id}`, {headers: otherHeaders})).status, 404);
    assert.equal((await fetch(`${base}/api/v1/mail/admin/providers`, {headers: otherHeaders})).status, 403);
    assert.equal((await fetch(`${base}/api/v1/mail/messages/flags`, {method: "POST", headers: otherHeaders, body: JSON.stringify({ids: [reply.id], seen: false})})).status, 404);
    // App passwords cannot use Mail.
    const appHeaders = {authorization: dav.authorization};
    assert.notEqual((await fetch(`${base}/api/v1/mail/overview`, {headers: appHeaders})).status, 200);

    // A provider removed from the host configuration stops its accounts with a reason.
    const reduced = JSON.parse(await readFile(providersFile, "utf8")).filter((p: {id: string}) => p.id !== "test-jmap");
    await writeFile(providersFile, JSON.stringify(reduced));
    await runtime.restart();
    const again = await login(base), againHeaders = {...again.headers, "content-type": "application/json"};
    await fetch(`${base}/api/v1/mail/sync`, {method: "POST", headers: againHeaders, body: JSON.stringify({id: jmapAccount.id})});
    const afterRestart = await (await fetch(base + "/api/v1/mail/overview", {headers: againHeaders})).json() as {accounts: (Account & {providerAvailable: boolean})[]; providers: {id: string}[]};
    assert.ok(!afterRestart.providers.some(p => p.id === "test-jmap"));
    const stopped = afterRestart.accounts.find(a => a.id === jmapAccount.id)!;
    assert.equal(stopped.providerAvailable, false); assert.match(stopped.lastError, /turned off/);
    // Mirrored mail stays readable without the server.
    assert.equal((await fetch(`${base}/api/v1/mail/messages/${reply.id}`, {headers: againHeaders})).status, 200);

    // Removing an account keeps its files unless asked.
    assert.equal((await fetch(`${base}/api/v1/mail/accounts/${jmapAccount.id}?keepFiles=1`, {method: "DELETE", headers: againHeaders})).status, 200);
    assert.equal((await fetch(`${base}/dav/files/admin/docs/mail/Personal/`, {method: "PROPFIND", headers: {...dav, depth: "0"}})).status, 207);

    // The administrator's switch turns the API off.
    const apps = await (await fetch(base + "/api/v1/appSettings", {headers: againHeaders})).json() as {revision: number};
    const off = await fetch(base + "/api/v1/admin/installedApps", {method: "PUT", headers: {...againHeaders, "if-match": `"apps:${apps.revision}"`}, body: JSON.stringify({id: "mail", enabled: false, revision: apps.revision})});
    assert.ok(off.status < 300, await off.text());
    assert.equal((await fetch(`${base}/api/v1/mail/overview`, {headers: againHeaders})).status, 404);
  } finally {
    await runtime.close();
    await server.close();
  }
});
