import type { IncomingMessage, ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";
import type { PimCollection, PimResource } from "../../shared/pim.ts";
import { AppError } from "../errors.ts";
import type { PimService } from "./service.ts";
import { pimEtag } from "./service.ts";
import { field, invalid } from "./standards.ts";
import { timezone } from "./calendar.ts";

export function requireMatch(req: IncomingMessage, row: { id: string; revision: number } | undefined): number {
  if (!row) { if (req.headers["if-none-match"] !== "*") throw new AppError("preconditionRequired", 428); return 0; }
  if (!req.headers["if-match"]) throw new AppError("preconditionRequired", 428);
  if (req.headers["if-match"] !== pimEtag(row)) throw new AppError("revisionConflict", 412);
  return row.revision;
}
export async function handlePimHttp(req: IncomingMessage, res: ServerResponse, url: URL, service: PimService, body: () => Promise<Record<string, unknown>>) {
  const method = req.method ?? "GET";
  const json = (value: unknown, status = 200, row?: { id: string; revision: number }) => { if (row) res.setHeader("etag", pimEtag(row)); res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" }); res.end(JSON.stringify(value, (key, value) => ["modifiedAt", "dueAt"].includes(key) && typeof value === "number" ? new Date(value).toISOString() : value)); };
  const send = (text: string, kind: string, filename: string) => { res.writeHead(200, { "content-type": kind === "contacts" ? "text/vcard; charset=utf-8" : "text/calendar; charset=utf-8", "content-disposition": `attachment; filename="${filename}"`, "cache-control": "no-store" }); res.end(text); };
  if (url.pathname === "/api/v1/pim/preferences") {
    const old = await service.call<{ revision: number; value: string }>("preferences");
    if (method === "GET") return json({ revision: old.revision, ...JSON.parse(old.value) }, 200, { id: "preferences", revision: old.revision });
    if (method === "PUT") {
      requireMatch(req, { id: "preferences", revision: old.revision }); const p = await body();
      const previous=JSON.parse(old.value),get=(key:string,fallback:unknown)=>p[key]===undefined?previous[key]??fallback:p[key];
      const zone = timezone(field(get("timezone","UTC"), 100)), weekStart = get("weekStart",1);
      if (weekStart !== 0 && weekStart !== 1) invalid("Week starts on Sunday or Monday.");
      if (p.hiddenCalendars !== undefined && (!Array.isArray(p.hiddenCalendars) || p.hiddenCalendars.length > 200 || p.hiddenCalendars.some(v => typeof v !== "string"))) invalid("Invalid hidden calendar list.");
      const choice=(key:string,allowed:string[],fallback:string)=>{const value=get(key,fallback);if(typeof value!=="string"||!allowed.includes(value))invalid(`Invalid ${key}.`);return value;};
      const flag=(key:string,fallback:boolean)=>{const value=get(key,fallback);if(typeof value!=="boolean")invalid(`Invalid ${key}.`);return value;};
      const value = JSON.stringify({calendarShowCancelled:flag("calendarShowCancelled",false),appearance:choice("appearance",["system","latte","mocha"],"system"),timezone:zone,weekStart,contactSort:choice("contactSort",["firstName","lastName"],"firstName"),hiddenCalendars:get("hiddenCalendars",[]),notesSort:choice("notesSort",["modified","title"],"modified"),tasksSort:choice("tasksSort",["modified","title","due"],"due"),filesView:choice("filesView",["list","grid"],"list"),filesShowHidden:flag("filesShowHidden",false),filesConfirmTrash:flag("filesConfirmTrash",false),filesDetailsTab:choice("filesDetailsTab",["details","versions","sharing"],"details"),notesEditorMode:choice("notesEditorMode",["write","preview"],"write"),tasksView:choice("tasksView",["all","inbox","today","upcoming","important","completed"],"today")});
      const result = await service.call<{ revision: number }>("savePreferences", { revision: old.revision, value }); return json({ ...JSON.parse(value), revision: result.revision });
    }
  }
  if (url.pathname === "/api/v1/pim/reminders" && method === "GET") return json({ items: await service.call("reminders") });
  const reminder = /^\/api\/v1\/pim\/reminders\/([^/]+)$/.exec(url.pathname);
  if (reminder && method === "DELETE") return json(await service.call("acknowledge", { id: decodeURIComponent(reminder[1]) }));
  if (url.pathname === "/api/v1/contacts/people" && method === "GET") {
    const result=await service.call<{items:unknown[];groups:string[];nextCursor:string}>("search",{kind:"contacts",collection:url.searchParams.get("book")??"",q:url.searchParams.get("q")??"",tag:url.searchParams.get("group")??"",favorite:url.searchParams.get("favorite")==="1",sort:url.searchParams.get("sort"),cursor:Buffer.from(url.searchParams.get("cursor")??"","base64url").toString("utf8")});
    return json({...result,nextCursor:result.nextCursor?Buffer.from(result.nextCursor).toString("base64url"):""});
  }
  const match = /^\/api\/v1\/(contacts|calendar)\/(books|calendars)(?:\/([a-f0-9-]{36}))?(?:\/(items|export|import|occurrences|grants)(?:\/([a-zA-Z0-9_.-]+))?)?$/.exec(url.pathname);
  if (!match || match[1] === "contacts" && match[2] !== "books" || match[1] === "calendar" && match[2] !== "calendars") throw new AppError("notFound", 404);
  const [, kind, , id, sub, href] = match;
  if (!id) {
    if (method === "GET") return json({ items: (await service.collections()).filter(c => c.kind === kind), nextCursor: "" });
    if (method === "POST") { const p = await body(); const row = await service.call<PimCollection>("createCollection", { id: randomUUID(), kind, name: field(p.name, 255), color: field(p.color ?? "#2563eb", 7) }); return json(row, 201, row); }
    throw new AppError("methodNotAllowed", 405);
  }
  const collection = (await service.collections()).find(c => c.id === id && c.kind === kind);
  if (!collection) throw new AppError("notFound", 404);
  if (!sub) {
    if (method === "GET") { const result = await service.list(id); return json({ ...collection, count: result.items.length }, 200, collection); }
    if (method === "PUT" || method === "DELETE") {
      const revision = requireMatch(req, collection);
      return json(await service.call(method === "PUT" ? "renameCollection" : "deleteCollection", { ...method === "PUT" ? await body() : {}, collectionId: id, revision }));
    }
  }
  if (sub === "grants") {
    if (method === "GET") return json({ items: await service.call("grants", { collectionId: id }) });
    if (method === "POST" || method === "DELETE") return json(await service.call(method === "POST" ? "share" : "revoke", { ...await body(), collectionId: id, revision: requireMatch(req, collection) }));
  }
  if (sub === "occurrences" && kind === "calendar" && method === "GET") return json({ items: await service.occurrences(id, url.searchParams.get("timezone") ?? "UTC", new Date(url.searchParams.get("from") ?? ""), new Date(url.searchParams.get("to") ?? ""), url.searchParams.get("includeCancelled") === "1"), nextCursor: "" });
  if (sub === "export" && method === "GET") return send(await service.exportText(id), kind, kind === "contacts" ? "contacts.vcf" : "calendar.ics");
  if (sub === "import" && method === "POST") { const p = await body(); return json(await service.importText(collection, field(p.content, 2*1024*1024))); }
  if (sub === "items") {
    if (href) {
      const old = await service.read(id, href);
      if (method === "GET") { if (url.searchParams.get("export") === "1") return send(old.content, kind, href); return json({ ...old, data: JSON.parse(old.projection) }, 200, old); }
      if (method === "DELETE") return json(await service.call("delete", { collectionId: id, href, revision: requireMatch(req, old) }));
      if (method === "PUT") { const row = await service.put(collection, href, await body(), requireMatch(req, old), old.content); return json({ ...row, data: JSON.parse(row.projection) }, 200, row); }
    } else {
      if (method === "POST") {
        const row = await service.put(collection, `${randomUUID()}.${kind === "contacts" ? "vcf" : "ics"}`, await body(), 0); return json({ ...row, data: JSON.parse(row.projection) }, 201, row);
      }
      if (method === "GET") {
        const result = await service.list(id), query = (url.searchParams.get("q") ?? "").normalize("NFC").toLocaleLowerCase(), cursor = url.searchParams.get("cursor") ?? "", group = url.searchParams.get("group"), favorite = url.searchParams.get("favorite") === "1";
        const items = result.items.map(({ content: _content, ...row }) => ({ ...row, data: JSON.parse(row.projection) })).filter(row => {
          const data = row.data; return row.id > cursor && (!query || JSON.stringify(data).normalize("NFC").toLocaleLowerCase().includes(query)) && (!group || data.groups?.includes(group)) && (!favorite || data.favorite);
        }).sort((a,b) => a.id.localeCompare(b.id));
        return json({ items: items.slice(0, 200), nextCursor: items.length > 200 ? items[199].id : "", groups: [...new Set(result.items.flatMap(row => (JSON.parse(row.projection).groups as string[] | undefined) ?? []))].sort() });
      }
    }
  }
  throw new AppError("methodNotAllowed", 405);
}
