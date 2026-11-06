import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { login, connect } from "./integration-helpers.ts";
import { database } from "../runtime/database.ts";
import { deliverReminders } from "../runtime/pim/reminders.ts";
import type { PimCollection, PimResource } from "../shared/pim.ts";
const origin = process.env.DAR_INTEGRATION_ORIGIN;
const etag = (row: { id: string; revision: number }) => `"${row.id}:${row.revision}"`;
type Session = Awaited<ReturnType<typeof login>>;
async function request(session: Session, path: string, method = "GET", body?: object, match?: string, expected = 200) {
  const r = await fetch(origin! + path, { method, headers: { ...session.headers, "content-type": "application/json", ...match ? { "if-match": match } : {} }, body: body ? JSON.stringify(body) : undefined });
  assert.equal(r.status, expected, `${method} ${path}: ${await r.clone().text()}`); return r;
}
test("Contacts native CRUD, imports, private isolation, live sharing revocation and revision conflicts", { skip: !origin, timeout: 30000 }, async () => {
  const admin = await login(origin!), user = `pim-${randomUUID().slice(0,8)}`;
  await request(admin, "/api/v1/auth/users", "POST", { username: user, displayName: "Other user", password: "a private contacts password" }, undefined, 201);
  const other = await login(origin!, user, "a private contacts password"), live = await connect(other);
  await new Promise<void>((resolve,reject) => live.subscriptionBuilder().onApplied(() => resolve()).onError(c => reject(c.event)).subscribe("SELECT * FROM my_pim_changes"));
  try {
    let book = await (await request(admin, "/api/v1/contacts/books", "POST", { name: "Personal", color: "#2563eb" }, undefined, 201)).json() as PimCollection;
    const path = `/api/v1/contacts/books/${book.id}`;
    const created = await (await request(admin, path + "/items", "POST", { data: { firstName: "Amira", lastName: "Lee", emails: [{ label: "work", value: "amira@example.com" }], groups: ["Design team"] } }, undefined, 201)).json() as PimResource;
    await request(other, path + "/items", "GET", undefined, undefined, 404);
    const changed = await (await request(admin, path + "/items/" + created.href, "PUT", { data: { displayName: "Amira Lee", favorite: true } }, etag(created))).json() as PimResource;
    await request(admin, path + "/items/" + created.href, "PUT", { data: { displayName: "Stale" } }, etag(created), 412);
    await request(admin, path + "/items/" + created.href, "DELETE", undefined, undefined, 428);
    book = await (await request(admin, path)).json() as PimCollection;
    await request(admin, path + "/grants", "POST", { username: user, role: "read" }, etag(book));
    assert.equal(((await (await request(other, path + "/items")).json()) as { items: unknown[] }).items.length, 1);
    await request(other, path + "/items/" + changed.href, "PUT", { data: { displayName: "No access" } }, etag(changed), 403);
    const grants = await (await request(admin, path + "/grants")).json() as { items: { id: string }[] };
    book = await (await request(admin, path)).json() as PimCollection;
    const notified = new Promise<void>(resolve => live.db.myPimChanges.onUpdate(() => resolve()));
    await request(admin, path + "/grants", "DELETE", { id: grants.items[0].id }, etag(book)); await notified;
    await request(other, path + "/items", "GET", undefined, undefined, 404);
    const card = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:external-1\r\nFN:External Contact\r\nX-KEEP:unknown\r\nEND:VCARD\r\n";
    const imported = await (await request(admin, path + "/import", "POST", { content: card + card })).json() as { imported: number; items: { error?: string }[] };
    assert.equal(imported.imported, 1); assert.match(imported.items[1].error!, /conflict/);
    const exported = await (await request(admin, path + "/export")).text(); assert.match(exported, /X-KEEP:unknown/);
    await request(admin, path + "/items/" + changed.href, "DELETE", undefined, etag(changed));
    await request(admin, path + "/items/" + changed.href, "GET", undefined, undefined, 404);
  } finally { live.disconnect(); }
});
test("DAV discovery, conditional native/DAV round trip, namespace filters and credential scoping", { skip: !origin, timeout: 30000 }, async () => {
  const session = await login(origin!);
  for (const kind of ["contacts", "calendar"] as const) {
    const base = `/api/v1/${kind}/${kind === "contacts" ? "books" : "calendars"}`;
    const collection = await (await request(session, base, "POST", { name: `DAV ${kind}`, color: "#2563eb" }, undefined, 201)).json() as PimCollection;
    const password = await (await request(session, "/api/v1/auth/app-passwords", "POST", { label: `DAV ${kind}`, scope: `${kind}:write` }, undefined, 201)).json() as { id: string; secret: string; davUrl: string };
    const headers = { authorization: `Basic ${Buffer.from(`admin:${password.secret}`).toString("base64")}` }, dav = password.davUrl + collection.id + "/", href = kind === "contacts" ? "test.vcf" : "test.ics";
    const content = kind === "contacts" ? "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:dav-contact\r\nFN:DAV Contact\r\nEND:VCARD\r\n" : "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Test//EN\r\nBEGIN:VEVENT\r\nUID:dav-event\r\nDTSTART:20261109T090000Z\r\nDTEND:20261109T100000Z\r\nSUMMARY:DAV Event\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    const discovery = await fetch(origin! + "/dav/", { method: "PROPFIND", headers: { ...headers, depth: "0" }, body: '<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>' }); assert.equal(discovery.status, 207); assert.match(await discovery.text(), /principals\/admin/);
    const put = await fetch(dav + href, { method: "PUT", headers: { ...headers, "if-none-match": "*" }, body: content }); assert.equal(put.status, 201, await put.clone().text());
    const tag = put.headers.get("etag")!;
    assert.equal((await fetch(dav + href, { method: "PUT", headers, body: content })).status, 428);
    const native = await (await request(session, `${base}/${collection.id}/items/${href}`)).json() as PimResource; assert.equal(etag(native), tag);
    const ns = kind === "contacts" ? "carddav" : "caldav", report = kind === "contacts" ? "addressbook-multiget" : "calendar-multiget", data = kind === "contacts" ? "address-data" : "calendar-data";
    const result = await fetch(dav, { method: "REPORT", headers, body: `<q:${report} xmlns:q="urn:ietf:params:xml:ns:${ns}" xmlns:d="DAV:"><d:prop><d:getetag/><q:${data}/></d:prop><d:href>${new URL(dav).pathname}${href}</d:href></q:${report}>` }); assert.equal(result.status, 207); assert.match(await result.text(), /DAV (Contact|Event)/);
    const filter = kind === "contacts" ? '<q:filter><q:prop-filter name="FN"><q:text-match collation="i;unicode-casemap">contact</q:text-match></q:prop-filter></q:filter>' : '<q:filter><q:comp-filter name="VCALENDAR"><q:comp-filter name="VEVENT"><q:time-range start="20261109T000000Z" end="20261110T000000Z"/></q:comp-filter></q:comp-filter></q:filter>';
    const query = await fetch(dav, { method: "REPORT", headers, body: `<q:${kind === "contacts" ? "addressbook-query" : "calendar-query"} xmlns:q="urn:ietf:params:xml:ns:${ns}" xmlns:d="DAV:"><d:prop><d:getetag/></d:prop>${filter}</q:${kind === "contacts" ? "addressbook-query" : "calendar-query"}>` });
    assert.equal(query.status,207); assert.match(await query.text(), new RegExp(href.replace(".","\\.")));
    assert.equal((await fetch(origin! + "/dav/files/admin/", { method: "PROPFIND", headers: { ...headers, depth: "0" } })).status, 403);
    assert.equal((await fetch(dav + href, { method: "DELETE", headers: { ...headers, "if-match": tag } })).status, 204);
    await request(session, "/api/v1/auth/app-passwords/" + password.id, "DELETE"); assert.equal((await fetch(dav, { method: "PROPFIND", headers: { ...headers, depth: "0" } })).status, 401);
  }
});
test("Calendar native recurrence and durable reminder acknowledgement/edit invalidation", { skip: !origin, timeout: 30000 }, async () => {
  const session = await login(origin!), db = await database(process.env.DAR_DATABASE_URL!, process.env.DAR_DATABASE_NAME!, process.env.DAR_DATABASE_TOKEN_FILE!);
  const collection = await (await request(session, "/api/v1/calendar/calendars", "POST", { name: "Personal calendar", color: "#24804b" }, undefined, 201)).json() as PimCollection, base = `/api/v1/calendar/calendars/${collection.id}`;
  const start = new Date(Date.now() + 3603000).toISOString().slice(0,19), end = new Date(Date.now() + 7203000).toISOString().slice(0,19);
  const event = await (await request(session, base + "/items", "POST", { data: { title: "Due reminder", start, end, timezone: "UTC", recurrence: "FREQ=DAILY;COUNT=3", reminderMinutes: 60 } }, undefined, 201)).json() as PimResource;
  const from = new Date(Date.now()-86400000).toISOString(), to = new Date(Date.now()+4*86400000).toISOString();
  const list = await (await request(session, base + `/occurrences?from=${from}&to=${to}&timezone=UTC`)).json() as { items: { recurrenceId: string }[] }; assert.equal(list.items.length, 3);
  const pin = randomUUID();
  const snapshot = await db.call<{ pim: { resources: { id: string; modifiedAt: number }[] } }>("service_backup", [pin]);
  assert.ok(snapshot.pim.resources.some(row => row.id === event.id));
  await db.call("service_release_backup", [pin]);
  // Wait for the actual due boundary, rather than an arbitrary polling sleep.
  await new Promise(resolve => setTimeout(resolve, Math.max(0, Date.parse(start + "Z") - 3600000 - Date.now() + 25)));
  await deliverReminders(db);
  type Inbox = { items: { id: string; resourceId: string }[] };
  let due = await (await request(session, "/api/v1/pim/reminders")).json() as Inbox;
  const reminder = due.items.find(row => row.resourceId === event.id); assert.ok(reminder);
  await deliverReminders(db);
  due = await (await request(session, "/api/v1/pim/reminders")).json() as Inbox;
  assert.equal(due.items.filter(row => row.resourceId === event.id).length, 1);
  await request(session, "/api/v1/pim/reminders/" + encodeURIComponent(reminder.id), "DELETE");
  due = await (await request(session, "/api/v1/pim/reminders")).json() as Inbox;
  assert.equal(due.items.some(row => row.resourceId === event.id), false);
  await request(session, base + "/items/" + event.href, "PUT", { data: { title: "Updated reminder", start, end, timezone: "UTC", recurrence: "FREQ=DAILY;COUNT=3", reminderMinutes: null } }, etag(event));
  await deliverReminders(db);
  const inbox = await (await request(session, "/api/v1/pim/reminders")).json() as { items: { resourceId: string }[] }; assert.equal(inbox.items.some(row => row.resourceId === event.id), false);
});
test("Contacts pagination walks a 250-card Unicode import without lost or duplicate rows", { skip: !origin, timeout: 30000 }, async () => {
  const session = await login(origin!);
  const book = await (await request(session,"/api/v1/contacts/books","POST",{name:"Pagination contacts",color:"#2563eb"},undefined,201)).json() as PimCollection;
  const content = Array.from({length:250},(_,i)=>`BEGIN:VCARD\r\nVERSION:4.0\r\nUID:page-${i}\r\nFN:Zoë ${String(i).padStart(3,"0")}\r\nCATEGORIES:Friends\r\nEND:VCARD\r\n`).join("");
  const imported = await (await request(session,`/api/v1/contacts/books/${book.id}/import`,"POST",{content})).json() as {imported:number}; assert.equal(imported.imported,250);
  const first = await (await request(session,`/api/v1/contacts/people?book=${book.id}&q=zoë&group=Friends`)).json() as {items:{id:string}[];nextCursor:string}; assert.equal(first.items.length,200); assert.ok(first.nextCursor);
  const second = await (await request(session,`/api/v1/contacts/people?book=${book.id}&q=zoë&group=Friends&cursor=${encodeURIComponent(first.nextCursor)}`)).json() as typeof first; assert.equal(second.items.length,50); assert.equal(second.nextCursor,"");
  assert.equal(new Set([...first.items,...second.items].map(row=>row.id)).size,250);
});
test("Reminder batches advance fairly beyond the first 200 resources", { skip: !origin, timeout: 30000 }, async () => {
  const session = await login(origin!), db = await database(process.env.DAR_DATABASE_URL!,process.env.DAR_DATABASE_NAME!,process.env.DAR_DATABASE_TOKEN_FILE!);
  const calendar = await (await request(session,"/api/v1/calendar/calendars","POST",{name:"Reminder batching",color:"#2563eb"},undefined,201)).json() as PimCollection;
  const content="BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Batch proof//EN\r\n"+Array.from({length:205},(_,i)=>`BEGIN:VEVENT\r\nUID:batch-${i}\r\nDTSTART:20261109T090000Z\r\nDTEND:20261109T100000Z\r\nSUMMARY:Batch ${i}\r\nEND:VEVENT\r\n`).join("")+"END:VCALENDAR\r\n";
  const imported=await(await request(session,`/api/v1/calendar/calendars/${calendar.id}/import`,"POST",{content})).json() as {imported:number;items:{href:string}[]};assert.equal(imported.imported,205);
  const last=await(await request(session,`/api/v1/calendar/calendars/${calendar.id}/items/${imported.items.at(-1)!.href}`)).json() as {modifiedAt:string};
  await new Promise(resolve=>setTimeout(resolve,Math.max(0,Date.parse(last.modifiedAt)+1005-Date.now())));
  type Work={resource:{id:string;collectionId:string};revision:number;from:number}[];
  const first=await db.call<Work>("service_pim_reminder_work");assert.equal(first.length,200);
  await deliverReminders(db);
  const next=await db.call<Work>("service_pim_reminder_work");assert.ok(next.some(job=>job.resource.collectionId===calendar.id));
  const ids=new Set(first.map(job=>job.resource.id));assert.equal(next.some(job=>ids.has(job.resource.id)),false);
  const current=await(await request(session,`/api/v1/calendar/calendars/${calendar.id}`)).json() as PimCollection;
  await request(session,`/api/v1/calendar/calendars/${calendar.id}`,"DELETE",undefined,etag(current));
});
