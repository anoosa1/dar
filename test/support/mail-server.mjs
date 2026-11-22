// A fake mail provider for Mail's tests: IMAP (993-style TLS), SMTP submission (465-style TLS),
// JMAP over HTTPS and an OAuth2 authorization server, all on 127.0.0.1 with a throwaway CA made
// here (no openssl needed). The runtime trusts it through SSL_CERT_FILE. State is in memory and
// exposed for assertions.
import {createHash, createPrivateKey, generateKeyPairSync, randomBytes, sign} from "node:crypto";
import {writeFileSync} from "node:fs";
import https from "node:https";
import path from "node:path";
import tls from "node:tls";

// ---------- A CA and a server certificate (DER by hand) ----------

const der = (tag, ...parts) => {
  const body = Buffer.concat(parts.map(p => Buffer.isBuffer(p) ? p : Buffer.from(p)));
  const n = body.length;
  const len = n < 128 ? Buffer.from([n]) : n < 256 ? Buffer.from([0x81, n]) : Buffer.from([0x82, n >> 8, n & 255]);
  return Buffer.concat([Buffer.from([tag]), len, body]);
};
const seq = (...p) => der(0x30, ...p), set = (...p) => der(0x31, ...p);
const oid = text => {
  const [a, b, ...rest] = text.split(".").map(Number);
  const bytes = [40 * a + b];
  for (const v of rest) { const out = [v & 127]; let x = v >>> 7; while (x) { out.unshift((x & 127) | 128); x >>>= 7; } bytes.push(...out); }
  return der(0x06, Buffer.from(bytes));
};
const int = value => { let b = Buffer.isBuffer(value) ? value : Buffer.from([value]); if (b[0] & 0x80) b = Buffer.concat([Buffer.from([0]), b]); return der(0x02, b); };
const utf8 = s => der(0x0c, Buffer.from(s));
const time = date => der(0x17, Buffer.from(date.toISOString().replace(/[-:T]/g, "").slice(2, 14) + "Z"));
const name = cn => seq(set(seq(oid("2.5.4.3"), utf8(cn))));
const ext = (id, critical, value) => seq(oid(id), ...(critical ? [der(0x01, Buffer.from([0xff]))] : []), der(0x04, value));
const ecdsa = seq(oid("1.2.840.10045.4.3.2"));

function certificate({subject, issuer, publicKey, signer, ca}) {
  const now = Date.now();
  const extensions = ca
    ? [ext("2.5.29.19", true, seq(der(0x01, Buffer.from([0xff])))), ext("2.5.29.15", true, der(0x03, Buffer.from([1, 0x06])))]
    : [ext("2.5.29.17", false, seq(der(0x82, Buffer.from("localhost")), der(0x87, Buffer.from([127, 0, 0, 1])))),
       ext("2.5.29.15", true, der(0x03, Buffer.from([7, 0x80]))), ext("2.5.29.37", false, seq(oid("1.3.6.1.5.5.7.3.1")))];
  const tbs = seq(der(0xa0, int(2)), int(randomBytes(8)), ecdsa, name(issuer), seq(time(new Date(now - 3600_000)), time(new Date(now + 86400_000))),
    name(subject), publicKey.export({type: "spki", format: "der"}), der(0xa3, seq(...extensions)));
  const signature = sign("sha256", tbs, {key: signer, dsaEncoding: "der"});
  return seq(tbs, ecdsa, der(0x03, Buffer.concat([Buffer.from([0]), signature])));
}
const pem = (label, body) => `-----BEGIN ${label}-----\n${body.toString("base64").match(/.{1,64}/g).join("\n")}\n-----END ${label}-----\n`;

export function makeCertificates(dir) {
  const ca = generateKeyPairSync("ec", {namedCurve: "P-256"}), leaf = generateKeyPairSync("ec", {namedCurve: "P-256"});
  const caCert = certificate({subject: "Dar test mail CA", issuer: "Dar test mail CA", publicKey: ca.publicKey, signer: ca.privateKey, ca: true});
  const leafCert = certificate({subject: "localhost", issuer: "Dar test mail CA", publicKey: leaf.publicKey, signer: ca.privateKey, ca: false});
  const caFile = path.join(dir, "mail-ca.pem");
  writeFileSync(caFile, pem("CERTIFICATE", caCert));
  return {caFile, cert: pem("CERTIFICATE", leafCert) + pem("CERTIFICATE", caCert), key: createPrivateKey(leaf.privateKey.export({type: "pkcs8", format: "pem"})).export({type: "pkcs8", format: "pem"})};
}

// ---------- Mailboxes ----------

export function message({from = "Lina Karam <lina@example.test>", to = "Anas <anas@example.test>", cc, subject, body = "Hello", date = new Date("2026-11-20T10:24:00Z"), id, html, attachment, references}) {
  const mid = id ?? `${randomBytes(6).toString("hex")}@example.test`;
  const head = [`From: ${from}`, `To: ${to}`, ...(cc ? [`Cc: ${cc}`] : []), `Subject: ${subject}`, `Date: ${date.toUTCString().replace("GMT", "+0000")}`, `Message-ID: <${mid}>`,
    ...(references ? [`References: ${references.map(r => `<${r}>`).join(" ")}`, `In-Reply-To: <${references.at(-1)}>`] : []), "MIME-Version: 1.0"];
  if (!html && !attachment) return Buffer.from([...head, "Content-Type: text/plain; charset=utf-8", "", body, ""].join("\r\n"));
  const parts = [`--b1\r\nContent-Type: ${html ? "text/html" : "text/plain"}; charset=utf-8\r\n\r\n${html ?? body}\r\n`];
  if (attachment) parts.push(`--b1\r\nContent-Type: ${attachment.type}; name="${attachment.name}"\r\nContent-Disposition: attachment; filename="${attachment.name}"\r\nContent-Transfer-Encoding: base64\r\n\r\n${Buffer.from(attachment.data).toString("base64")}\r\n`);
  return Buffer.from([...head, "Content-Type: multipart/mixed; boundary=b1", "", parts.join("") + "--b1--", ""].join("\r\n"));
}

function store() {
  const boxes = new Map();
  let validity = 1000;
  const add = (name, special = "") => boxes.set(name, {name, special, validity: ++validity, next: 1, messages: []});
  add("INBOX"); add("Sent", "\\Sent"); add("Drafts", "\\Drafts"); add("Trash", "\\Trash"); add("Archive", "\\Archive"); add("Junk", "\\Junk"); add("Projects");
  let jmapIds = 0;
  return {
    boxes,
    add,
    put(box, data, flags = []) {
      const b = boxes.get(box); const m = {uid: b.next++, flags: new Set(flags), data: Buffer.from(data), id: `e${++jmapIds}`}; b.messages.push(m); return m;
    },
    find(predicate) { for (const b of boxes.values()) for (const m of b.messages) if (predicate(m, b)) return {box: b, message: m}; return null; },
    text(m) { return m.data.toString("utf8"); },
  };
}

// ---------- IMAP ----------

function imapString(s) { return `"${s.replace(/\\/g, "\\\\").replace(/"/g, "\\\"")}"`; }
function uidSet(text, all) {
  const out = new Set();
  for (const part of text.split(",")) {
    const [a, b] = part.split(":");
    const max = all.length ? Math.max(...all) : 0;
    const lo = a === "*" ? max : Number(a), hi = b === undefined ? lo : b === "*" ? max : Number(b);
    for (const u of all) if (u >= Math.min(lo, hi) && u <= Math.max(lo, hi)) out.add(u);
  }
  return [...out];
}
function tokens(line) {
  const out = []; let i = 0;
  while (i < line.length) {
    if (line[i] === " ") { i++; continue; }
    if (line[i] === "\"") { let s = ""; i++; while (i < line.length && line[i] !== "\"") { if (line[i] === "\\") i++; s += line[i++]; } i++; out.push(s); continue; }
    if (line[i] === "(") { const end = line.indexOf(")", i); out.push(line.slice(i, end + 1)); i = end + 1; continue; }
    let j = i; while (j < line.length && line[j] !== " ") j++; out.push(line.slice(i, j)); i = j;
  }
  return out;
}

function imapServer(state, options) {
  return tls.createServer({cert: options.cert, key: options.key}, socket => {
    state.imapConnections++;
    let buffer = Buffer.alloc(0), user = null, selected = null, pending = null;
    const send = text => socket.write(text);
    send("* OK Dar fake IMAP ready\r\n");
    const caps = "CAPABILITY IMAP4rev1 AUTH=PLAIN AUTH=XOAUTH2 SASL-IR MOVE UIDPLUS LITERAL+ SPECIAL-USE";
    const authenticate = (mechanism, data) => {
      const raw = Buffer.from(data, "base64").toString("utf8");
      if (mechanism === "PLAIN") { const [, u, p] = raw.split("\0"); return state.users[u]?.password === p ? u : null; }
      const m = /^user=([^\x01]*)\x01auth=Bearer ([^\x01]*)\x01\x01$/.exec(raw);
      return m && state.oauth.access.has(m[2]) && state.oauth.access.get(m[2]) === m[1] ? m[1] : null;
    };
    const handle = (line, literal) => {
      const [tag, rawCommand, ...rest] = tokens(line);
      let command = (rawCommand ?? "").toUpperCase();
      let args = rest;
      if (command === "UID") { command = "UID " + (args[0] ?? "").toUpperCase(); args = args.slice(1); }
      state.imapCommands.push(command);
      const ok = (text = "done", code = "") => send(`${tag} OK ${code ? `[${code}] ` : ""}${text}\r\n`);
      const no = text => send(`${tag} NO ${text}\r\n`);
      if (command === "CAPABILITY") { send(`* ${caps}\r\n`); return ok(); }
      if (command === "NOOP") return ok();
      if (command === "LOGOUT") { send("* BYE\r\n"); ok(); return socket.end(); }
      if (command === "LOGIN") { if (state.users[args[0]]?.password === args[1]) { user = args[0]; return ok(); } return no("[AUTHENTICATIONFAILED] Invalid credentials"); }
      if (command === "AUTHENTICATE") {
        const u = authenticate(args[0].toUpperCase(), args[1] ?? "");
        if (u) { user = u; return ok(); }
        return no("[AUTHENTICATIONFAILED] Invalid credentials");
      }
      if (!user) return no("Sign in first");
      if (command === "LIST") {
        if (args[1] === "") { send(`* LIST (\\Noselect) "/" ""\r\n`); return ok(); }
        for (const b of state.mail.boxes.values()) send(`* LIST (${[b.special, b.name.includes("/") || [...state.mail.boxes.keys()].some(k => k.startsWith(b.name + "/")) ? "\\HasChildren" : "\\HasNoChildren"].filter(Boolean).join(" ")}) "/" ${imapString(b.name)}\r\n`);
        return ok();
      }
      if (command === "SUBSCRIBE") return ok();
      if (command === "SELECT" || command === "EXAMINE") {
        const b = state.mail.boxes.get(args[0]); if (!b) { selected = null; return no("No such mailbox"); }
        selected = b; send(`* ${b.messages.length} EXISTS\r\n* OK [UIDVALIDITY ${b.validity}] UIDs valid\r\n* OK [UIDNEXT ${b.next}] next\r\n`); return ok("selected", "READ-WRITE");
      }
      if (command === "CREATE") { if (state.mail.boxes.has(args[0])) return no("[ALREADYEXISTS] exists"); state.mail.add(args[0]); state.created.push(args[0]); return ok(); }
      if (command === "RENAME") { const b = state.mail.boxes.get(args[0]); if (!b) return no("missing"); state.mail.boxes.delete(args[0]); b.name = args[1]; state.mail.boxes.set(args[1], b); return ok(); }
      if (command === "APPEND") {
        const b = state.mail.boxes.get(args[0]); if (!b) return no("[TRYCREATE] No such mailbox");
        const flags = args[1]?.startsWith("(") ? args[1].slice(1, -1).split(" ").filter(Boolean) : [];
        const m = state.mail.put(args[0], literal, flags); state.appended.push({box: args[0], flags, data: literal.toString("utf8")});
        return ok("appended", `APPENDUID ${b.validity} ${m.uid}`);
      }
      if (!selected) return no("Select a mailbox first");
      const all = selected.messages.map(m => m.uid);
      if (command === "UID FETCH") {
        const uids = uidSet(args[0], all), what = args.slice(1).join(" ").toUpperCase();
        for (const [n, m] of selected.messages.entries()) {
          if (!uids.includes(m.uid)) continue;
          if (what.includes("BODY.PEEK[]")) { socket.write(`* ${n + 1} FETCH (UID ${m.uid} BODY[] {${m.data.length}}\r\n`); socket.write(m.data); socket.write(")\r\n"); }
          else send(`* ${n + 1} FETCH (UID ${m.uid} FLAGS (${[...m.flags].join(" ")}) RFC822.SIZE ${m.data.length})\r\n`);
        }
        return ok();
      }
      if (command === "UID STORE") {
        const uids = uidSet(args[0], all), op = args[1].toUpperCase(), flags = args[2].slice(1, -1).split(" ").filter(Boolean);
        for (const m of selected.messages) if (uids.includes(m.uid)) for (const f of flags) op.startsWith("+") ? m.flags.add(f) : m.flags.delete(f);
        return ok();
      }
      if (command === "UID MOVE" || command === "UID COPY") {
        const target = state.mail.boxes.get(args[1]); if (!target) return no("[TRYCREATE] No such mailbox");
        const uids = uidSet(args[0], all), from = [], to = [];
        for (const m of [...selected.messages]) {
          if (!uids.includes(m.uid)) continue;
          const copy = state.mail.put(target.name, m.data, [...m.flags]); from.push(m.uid); to.push(copy.uid);
          if (command === "UID MOVE") selected.messages.splice(selected.messages.indexOf(m), 1);
        }
        state.moves.push({from: selected.name, to: target.name, count: from.length});
        if (command === "UID MOVE") { send(`* OK [COPYUID ${target.validity} ${from.join(",")} ${to.join(",")}] moved\r\n`); return ok(); }
        return ok("copied", `COPYUID ${target.validity} ${from.join(",")} ${to.join(",")}`);
      }
      if (command === "UID EXPUNGE" || command === "EXPUNGE") {
        const uids = command === "UID EXPUNGE" ? uidSet(args[0], all) : all;
        const before = selected.messages.length;
        selected.messages = selected.messages.filter(m => !(uids.includes(m.uid) && m.flags.has("\\Deleted")));
        state.expunged += before - selected.messages.length;
        return ok();
      }
      return send(`${tag} BAD Unknown command ${command}\r\n`);
    };
    socket.on("data", chunk => {
      buffer = Buffer.concat([buffer, chunk]);
      for (;;) {
        if (pending) {
          if (buffer.length < pending.size + 2) return;
          const literal = buffer.subarray(0, pending.size); buffer = buffer.subarray(pending.size);
          const end = buffer.indexOf("\r\n"); if (end < 0) { buffer = Buffer.concat([Buffer.from(""), buffer]); return; }
          buffer = buffer.subarray(end + 2);
          const line = pending.line; pending = null; handle(line, literal); continue;
        }
        const end = buffer.indexOf("\r\n"); if (end < 0) return;
        const line = buffer.subarray(0, end).toString("utf8"); buffer = buffer.subarray(end + 2);
        const literal = /\{(\d+)(\+?)\}$/.exec(line);
        if (literal) { pending = {size: Number(literal[1]), line: line.slice(0, literal.index).trimEnd()}; if (!literal[2]) send("+ go ahead\r\n"); continue; }
        handle(line, null);
      }
    });
    socket.on("error", () => {});
  });
}

// ---------- SMTP ----------

function smtpServer(state, options) {
  return tls.createServer({cert: options.cert, key: options.key}, socket => {
    let buffer = "", user = null, from = null, rcpt = [], data = null;
    const send = text => socket.write(text + "\r\n");
    send("220 Dar fake SMTP");
    socket.on("data", chunk => {
      buffer += chunk.toString("binary");
      for (;;) {
        if (data !== null) {
          const end = buffer.indexOf("\r\n.\r\n"); if (end < 0) return;
          data += buffer.slice(0, end); buffer = buffer.slice(end + 5);
          state.sent.push({user, from, to: rcpt, data: Buffer.from(data.replace(/\r\n\.\./g, "\r\n."), "binary").toString("utf8")}); data = null; rcpt = [];
          send("250 queued"); continue;
        }
        const end = buffer.indexOf("\r\n"); if (end < 0) return;
        const line = buffer.slice(0, end); buffer = buffer.slice(end + 2);
        const upper = line.toUpperCase();
        if (upper.startsWith("EHLO")) { send("250-fake"); send("250-AUTH PLAIN LOGIN XOAUTH2"); send("250 8BITMIME"); }
        else if (upper.startsWith("AUTH PLAIN ")) { const [, u, p] = Buffer.from(line.slice(11), "base64").toString().split("\0"); if (state.users[u]?.password === p) { user = u; send("235 ok"); } else send("535 bad credentials"); }
        else if (upper.startsWith("AUTH XOAUTH2 ")) { const m = /^user=([^\x01]*)\x01auth=Bearer ([^\x01]*)/.exec(Buffer.from(line.slice(13), "base64").toString()); if (m && state.oauth.access.get(m[2]) === m[1]) { user = m[1]; send("235 ok"); } else send("535 bad token"); }
        else if (!user && !upper.startsWith("QUIT")) send("530 sign in first");
        else if (upper.startsWith("MAIL FROM:")) { from = /<(.*)>/.exec(line)[1]; send("250 ok"); }
        else if (upper.startsWith("RCPT TO:")) { rcpt.push(/<(.*)>/.exec(line)[1]); send("250 ok"); }
        else if (upper === "DATA") { data = ""; send("354 go"); }
        else if (upper === "QUIT") { send("221 bye"); socket.end(); }
        else send("500 what");
      }
    });
    socket.on("error", () => {});
  });
}

// ---------- JMAP and OAuth over HTTPS ----------

function httpsServer(state, options, base) {
  const json = (res, status, value) => { res.writeHead(status, {"content-type": "application/json"}); res.end(JSON.stringify(value)); };
  const body = req => new Promise(resolve => { const parts = []; req.on("data", c => parts.push(c)); req.on("end", () => resolve(Buffer.concat(parts))); });
  const jmap = state.jmap;
  const email = (id) => jmap.emails.get(id);
  const keywords = m => Object.fromEntries([...m.keywords].map(k => [k, true]));
  return https.createServer({cert: options.cert, key: options.key}, async (req, res) => {
    const url = new URL(req.url, base());
    if (url.pathname === "/oauth/authorize") {
      state.oauth.authorizations.push(Object.fromEntries(url.searchParams));
      const code = randomBytes(8).toString("hex");
      state.oauth.codes.set(code, {challenge: url.searchParams.get("code_challenge"), redirect: url.searchParams.get("redirect_uri"), user: url.searchParams.get("login_hint")});
      res.writeHead(302, {location: `${url.searchParams.get("redirect_uri")}?code=${code}&state=${encodeURIComponent(url.searchParams.get("state"))}`}); return res.end();
    }
    if (url.pathname === "/oauth/token") {
      const form = new URLSearchParams((await body(req)).toString()); state.oauth.tokenRequests.push(Object.fromEntries(form));
      if (form.get("client_id") !== "dar-test-client" || form.get("client_secret") !== state.oauth.secret) return json(res, 401, {error: "invalid_client"});
      let user;
      if (form.get("grant_type") === "authorization_code") {
        const pending = state.oauth.codes.get(form.get("code")); state.oauth.codes.delete(form.get("code"));
        const challenge = createHash("sha256").update(form.get("code_verifier") ?? "").digest("base64url");
        if (!pending || pending.challenge !== challenge || pending.redirect !== form.get("redirect_uri")) return json(res, 400, {error: "invalid_grant"});
        user = pending.user;
      } else if (form.get("grant_type") === "refresh_token") {
        user = state.oauth.refresh.get(form.get("refresh_token")); if (!user) return json(res, 400, {error: "invalid_grant"});
      } else return json(res, 400, {error: "unsupported_grant_type"});
      const access = "at-" + randomBytes(8).toString("hex"), refresh = "rt-" + randomBytes(8).toString("hex");
      state.oauth.access.set(access, user); state.oauth.refresh.set(refresh, user);
      return json(res, 200, {access_token: access, refresh_token: refresh, expires_in: state.oauth.expiresIn, token_type: "Bearer"});
    }
    if (!url.pathname.startsWith("/jmap/")) { res.writeHead(404); return res.end(); }
    if (req.headers.authorization !== `Bearer ${jmap.token}`) return json(res, 401, {type: "unauthorized"});
    if (url.pathname === "/jmap/session") return json(res, 200, {apiUrl: "/jmap/api", downloadUrl: "/jmap/download/{accountId}/{blobId}/{name}?type={type}", uploadUrl: "/jmap/upload/{accountId}/",
      primaryAccounts: {"urn:ietf:params:jmap:mail": "A1"}, capabilities: {}, accounts: {A1: {}}});
    if (url.pathname.startsWith("/jmap/download/")) {
      const blob = jmap.blobs.get(url.pathname.split("/")[4]); if (!blob) { res.writeHead(404); return res.end(); }
      res.writeHead(200, {"content-type": "message/rfc822"}); return res.end(blob);
    }
    if (url.pathname.startsWith("/jmap/upload/")) { const id = "B" + randomBytes(6).toString("hex"); jmap.blobs.set(id, await body(req)); return json(res, 201, {blobId: id, accountId: "A1"}); }
    if (url.pathname !== "/jmap/api") { res.writeHead(404); return res.end(); }
    const request = JSON.parse((await body(req)).toString());
    const responses = request.methodCalls.map(([name, args, id]) => {
      jmap.calls.push(name);
      if (name === "Mailbox/get") return [name, {list: [...jmap.mailboxes.values()]}, id];
      if (name === "Email/query") { const ids = [...jmap.emails.values()].filter(e => e.mailboxIds[args.filter.inMailbox]).map(e => e.id); return [name, {ids: ids.slice(args.position, args.position + args.limit), total: ids.length}, id]; }
      if (name === "Email/get") return [name, {list: args.ids.map(email).filter(Boolean).map(e => ({id: e.id, blobId: e.blobId, size: jmap.blobs.get(e.blobId).length, keywords: keywords(e), mailboxIds: e.mailboxIds}))}, id];
      if (name === "Email/set") {
        for (const [eid, patch] of Object.entries(args.update ?? {})) {
          const e = email(eid); if (!e) continue;
          for (const [key, value] of Object.entries(patch)) {
            const [field, item] = key.split("/");
            if (field === "keywords") value ? e.keywords.add(item) : e.keywords.delete(item);
            if (field === "mailboxIds") value ? e.mailboxIds[item] = true : delete e.mailboxIds[item];
          }
        }
        for (const eid of args.destroy ?? []) jmap.emails.delete(eid);
        return [name, {updated: Object.fromEntries(Object.keys(args.update ?? {}).map(k => [k, null])), destroyed: args.destroy ?? []}, id];
      }
      if (name === "Email/import") {
        const created = {};
        for (const [k, spec] of Object.entries(args.emails)) { const e = {id: "e" + randomBytes(4).toString("hex"), blobId: spec.blobId, mailboxIds: {...spec.mailboxIds}, keywords: new Set(Object.keys(spec.keywords ?? {}))}; jmap.emails.set(e.id, e); created[k] = {id: e.id}; }
        return [name, {created}, id];
      }
      if (name === "Mailbox/set") {
        const created = {};
        for (const [k, spec] of Object.entries(args.create ?? {})) { const mid = "m" + randomBytes(3).toString("hex"); jmap.mailboxes.set(mid, {id: mid, name: spec.name, parentId: spec.parentId, role: null}); created[k] = {id: mid}; }
        for (const [mid, patch] of Object.entries(args.update ?? {})) Object.assign(jmap.mailboxes.get(mid), patch);
        return [name, {created, updated: {}}, id];
      }
      if (name === "Identity/get") return [name, {list: [{id: "I1", email: jmap.address, name: "JMAP Tester"}]}, id];
      if (name === "EmailSubmission/set") { const s = args.create.s; jmap.submissions.push({...s, data: jmap.blobs.get(email(s.emailId).blobId).toString()}); return [name, {created: {s: {id: "S1"}}}, id]; }
      return ["error", {type: "unknownMethod"}, id];
    });
    return json(res, 200, {methodResponses: responses});
  });
}

export async function startMailServer(dir) {
  const certs = makeCertificates(dir);
  const state = {
    users: {"anas@example.test": {password: "imap secret password"}, "oauth@example.test": {password: "unused"}},
    mail: store(), sent: [], appended: [], moves: [], created: [], expunged: 0, imapCommands: [], imapConnections: 0,
    oauth: {secret: "dar-test-client-secret", codes: new Map(), access: new Map(), refresh: new Map(), authorizations: [], tokenRequests: [], expiresIn: 3600},
    jmap: {token: "jmap-test-token", address: "jmap@example.test", mailboxes: new Map([["mi", {id: "mi", name: "Inbox", role: "inbox", parentId: null}], ["ms", {id: "ms", name: "Sent", role: "sent", parentId: null}], ["mt", {id: "mt", name: "Trash", role: "trash", parentId: null}], ["ma", {id: "ma", name: "Archive", role: "archive", parentId: null}]]),
      emails: new Map(), blobs: new Map(), calls: [], submissions: []},
  };
  state.jmap.add = (mailbox, data, keywords = []) => { const blobId = "B" + randomBytes(6).toString("hex"); state.jmap.blobs.set(blobId, Buffer.from(data)); const e = {id: "e" + randomBytes(4).toString("hex"), blobId, mailboxIds: {[mailbox]: true}, keywords: new Set(keywords)}; state.jmap.emails.set(e.id, e); return e; };
  const listen = server => new Promise(resolve => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));
  let httpsPort = 0;
  const imap = imapServer(state, certs), smtp = smtpServer(state, certs), web = httpsServer(state, certs, () => `https://127.0.0.1:${httpsPort}`);
  const [imapPort, smtpPort] = [await listen(imap), await listen(smtp)];
  httpsPort = await listen(web);
  return {
    state, caFile: certs.caFile, imapPort, smtpPort, httpsPort, https: `https://127.0.0.1:${httpsPort}`,
    close: () => Promise.all([imap, smtp, web].map(s => new Promise(r => { s.close(() => r()); s.closeAllConnections?.(); }))),
  };
}
