import type { IncomingMessage, ServerResponse } from "node:http";
import { SaxesParser } from "saxes";
import type { Database } from "../database.ts";
import { digestToken } from "../auth/session.ts";
import { AppError } from "../errors.ts";
import { pimEtag, pimService } from "./service.ts";
import { requireMatch } from "./http.ts";
import { expand } from "./calendar.ts";
import { parsed } from "./standards.ts";
import {isTask,taskMatchesRange} from "./tasks.ts";

const D = "DAV:", A = "urn:ietf:params:xml:ns:carddav", C = "urn:ietf:params:xml:ns:caldav";
const escape = (value: unknown) => String(value).replace(/[<>&"']/g, c => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", '"': "&quot;", "'": "&apos;" })[c]!);
interface Xml { uri: string; name: string; attrs: Record<string, string>; text: string; children: Xml[] }
function xml(text: string): Xml {
  const stack: Xml[] = []; let root: Xml | undefined, nodes = 0;
  const parser = new SaxesParser({ xmlns: true });
  parser.on("doctype", () => { throw new AppError("invalidXml"); });
  parser.on("error", () => { throw new AppError("invalidXml"); });
  parser.on("opentag", tag => {
    if (++nodes > 10000 || stack.length > 32) throw new AppError("requestTooLarge", 413);
    const node = { uri: tag.uri, name: tag.local, attrs: Object.fromEntries(Object.values(tag.attributes).filter(a => !a.uri).map(a => [a.local, a.value])), text: "", children: [] } as Xml;
    if (stack.length) stack.at(-1)!.children.push(node); else { if (root) throw new AppError("invalidXml"); root = node; }
    stack.push(node);
  });
  parser.on("text", text => { if (stack.length) stack.at(-1)!.text += text; });
  parser.on("cdata", text => { if (stack.length) stack.at(-1)!.text += text; });
  parser.on("closetag", () => { stack.pop(); });
  try { parser.write(text).close(); } catch (error) { if (error instanceof AppError) throw error; throw new AppError("invalidXml"); }
  if (!root) throw new AppError("invalidXml"); return root;
}
const children = (node: Xml | undefined, uri: string, name: string) => node?.children.filter(n => n.uri === uri && n.name === name) ?? [];
async function readBody(req: IncomingMessage, limit = 262144) {
  const chunks: Buffer[] = []; let size = 0;
  for await (const chunk of req.iterator({ destroyOnReturn: false })) { size += chunk.length; if (size > limit) { req.resume(); throw new AppError("requestTooLarge", 413); } chunks.push(chunk); }
  return Buffer.concat(chunks).toString("utf8");
}
function davDate(value: string | undefined): Date {
  if (!value || !/^\d{8}T\d{6}Z$/.test(value)) throw new AppError("invalidTimeRange");
  const date = new Date(value.replace(/^(\d{4})(\d\d)(\d\d)T(\d\d)(\d\d)(\d\d)Z$/, "$1-$2-$3T$4:$5:$6Z"));
  if (!Number.isFinite(+date)) throw new AppError("invalidTimeRange"); return date;
}

/** CardDAV and CalDAV use the same checked domain operations as native edits. */
export async function handlePimDav(req: IncomingMessage, res: ServerResponse, db: Database, origin: string) {
  const method = req.method ?? "GET", url = new URL(req.url!, origin);
  if (req.headers.origin && req.headers.origin !== origin) throw new AppError("invalidOrigin", 403);
  res.setHeader("cache-control", "no-store");
  res.setHeader("allow", "OPTIONS, PROPFIND, REPORT, GET, HEAD, PUT, DELETE");
  if (method === "OPTIONS") { res.writeHead(204); res.end(); return; }
  const authorization = req.headers.authorization;
  if (!authorization?.startsWith("Basic ") || authorization.length > 2048) throw new AppError("unauthenticated", 401);
  const decoded = Buffer.from(authorization.slice(6), "base64").toString("utf8"), split = decoded.indexOf(":");
  if (split < 1) throw new AppError("unauthenticated", 401);
  const username = decoded.slice(0, split), secret = decoded.slice(split+1);
  if (!/^[A-Za-z0-9_-]{43}$/.test(secret)) throw new AppError("unauthenticated", 401);
  const write = ["PUT", "DELETE"].includes(method);
  let kind: "contacts" | "calendar" = url.pathname.startsWith("/dav/calendars/") ? "calendar" : "contacts";
  async function authenticate(app: string) { return db.call<{ sessionId: string; username: string }>("service_pim_authenticate", [username, digestToken(secret), app, write]); }
  let authenticated;
  try { authenticated = await authenticate(kind); }
  catch (error) { if (url.pathname !== "/dav/" && !url.pathname.startsWith("/dav/principals/")) throw error; kind = "calendar"; authenticated = await authenticate(kind); }
  const service = pimService(db, authenticated.sessionId), user = encodeURIComponent(authenticated.username), home = `/dav/${kind === "contacts" ? "addressbooks" : "calendars"}/${user}/`, principal = `/dav/principals/${user}/`;
  const collections = (await service.collections()).filter(c => c.kind === kind);
  const atHome = url.pathname === home, atRoot = ["/dav/", "/dav/addressbooks/", "/dav/calendars/"].includes(url.pathname), atPrincipal = url.pathname === principal;
  const tail = url.pathname.startsWith(home) ? url.pathname.slice(home.length).split("/").filter(Boolean) : [];
  if (!atRoot && !atPrincipal && !atHome && (!tail.length || tail.length > 2)) throw new AppError("notFound", 404);
  const collection = tail[0] ? collections.find(c => c.id === tail[0]) : undefined;
  if (tail.length && !collection) throw new AppError("notFound", 404);
  const href = tail[1], path = collection ? `${home}${collection.id}/` : url.pathname;
  const info = collection ? await service.list(collection.id) : undefined;
  const resource = href ? info?.items.find(r => r.href === href) : undefined;
  function props(name: string, isCollection: boolean, row?: { id: string; revision: number; modifiedAt: number | string; content?: string }) {
    const propertyCollection = row ? collections.find(c => c.id === row.id) : undefined;
    const map = new Map<string, string>([
      [`${D}|displayname`, `<d:displayname>${escape(name)}</d:displayname>`],
      [`${D}|resourcetype`, `<d:resourcetype>${isCollection ? "<d:collection/>" : ""}${isCollection && propertyCollection ? kind === "contacts" ? "<a:addressbook/>" : "<c:calendar/>" : ""}${atPrincipal ? "<d:principal/>" : ""}</d:resourcetype>`],
      [`${D}|current-user-principal`, `<d:current-user-principal><d:href>${principal}</d:href></d:current-user-principal>`],
      [`${A}|addressbook-home-set`, `<a:addressbook-home-set><d:href>/dav/addressbooks/${user}/</d:href></a:addressbook-home-set>`],
      [`${C}|calendar-home-set`, `<c:calendar-home-set><d:href>/dav/calendars/${user}/</d:href></c:calendar-home-set>`],
    ]);
    if (row) { map.set(`${D}|getetag`, `<d:getetag>${escape(pimEtag(row))}</d:getetag>`); map.set(`${D}|getlastmodified`, `<d:getlastmodified>${new Date(row.modifiedAt).toUTCString()}</d:getlastmodified>`); }
    if (row?.content) { map.set(`${D}|getcontenttype`, `<d:getcontenttype>${kind === "contacts" ? "text/vcard" : "text/calendar"}</d:getcontenttype>`); map.set(`${D}|getcontentlength`, `<d:getcontentlength>${Buffer.byteLength(row.content)}</d:getcontentlength>`); map.set(`${kind === "contacts" ? A : C}|${kind === "contacts" ? "address-data" : "calendar-data"}`, kind === "contacts" ? `<a:address-data>${escape(row.content)}</a:address-data>` : `<c:calendar-data>${escape(row.content)}</c:calendar-data>`); }
    if (isCollection && propertyCollection) {
      const ns = kind === "contacts" ? "a" : "c", reports = kind === "contacts" ? ["addressbook-query", "addressbook-multiget"] : ["calendar-query", "calendar-multiget"];
      map.set(`${D}|supported-report-set`, `<d:supported-report-set>${reports.map(report => `<d:supported-report><d:report><${ns}:${report}/></d:report></d:supported-report>`).join("")}</d:supported-report-set>`);
      map.set("http://calendarserver.org/ns/|getctag", `<cs:getctag>${escape(pimEtag(propertyCollection))}</cs:getctag>`);
      if (kind === "calendar") map.set(`${C}|supported-calendar-component-set`, '<c:supported-calendar-component-set><c:comp name="VEVENT"/><c:comp name="VTODO"/></c:supported-calendar-component-set>');
    }
    return map;
  }
  let request: Xml | undefined;
  if (method === "PROPFIND" || method === "REPORT") { const body = await readBody(req); if (body.trim()) request = xml(body); }
  const requested = children(request, D, "prop")[0]?.children;
  function response(href: string, properties: Map<string, string>) {
    const found: string[] = [], missing: string[] = [];
    for (const property of requested ?? [...properties.keys()].filter(key => !key.endsWith("-data")).map(key => { const at = key.lastIndexOf("|"); return { uri: key.slice(0,at), name: key.slice(at+1) }; })) {
      const content = properties.get(`${property.uri}|${property.name}`);
      if (content) found.push(content); else missing.push(`<x:${property.name} xmlns:x="${escape(property.uri)}"/>`);
    }
    return `<d:response><d:href>${escape(href)}</d:href>${found.length ? `<d:propstat><d:prop>${found.join("")}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>` : ""}${missing.length ? `<d:propstat><d:prop>${missing.join("")}</d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>` : ""}</d:response>`;
  }
  function multi(responses: string[]) { res.writeHead(207, { "content-type": "application/xml; charset=utf-8" }); res.end(`<?xml version="1.0" encoding="utf-8"?><d:multistatus xmlns:d="DAV:" xmlns:a="${A}" xmlns:c="${C}" xmlns:cs="http://calendarserver.org/ns/">${responses.join("")}</d:multistatus>`); }
  if (method === "PROPFIND") {
    if (!["0", "1"].includes(String(req.headers.depth ?? "1"))) throw new AppError("finiteDepthRequired", 403);
    if (href && !resource) throw new AppError("notFound", 404);
    const result = [response(url.pathname, props(resource?.href ?? collection?.name ?? authenticated.username, !href, resource ?? collection))];
    if (req.headers.depth !== "0" && !href) {
      if (collection) for (const row of info!.items) result.push(response(path + row.href, props(row.href, false, row)));
      else if (atHome) for (const c of collections) result.push(response(home + c.id + "/", props(c.name, true, c)));
    }
    return multi(result);
  }
  if (method === "REPORT") {
    if (!collection || href || !request) throw new AppError("invalidReport", 403);
    const ns = kind === "contacts" ? A : C, prefix = kind === "contacts" ? "addressbook" : "calendar";
    if (request.uri !== ns || ![`${prefix}-query`, `${prefix}-multiget`].includes(request.name)) throw new AppError("unsupportedReport", 403);
    let rows = info!.items;
    if (request.name.endsWith("multiget")) {
      const responses = [];
      for (const node of children(request, D, "href")) {
        const target = new URL(node.text, origin), name = target.pathname.slice(path.length);
        if (target.origin !== origin || !target.pathname.startsWith(path) || name.includes("/")) throw new AppError("invalidHref");
        const row = rows.find(r => r.href === name);
        responses.push(row ? response(target.pathname, props(row.href, false, row)) : `<d:response><d:href>${escape(target.pathname)}</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>`);
      }
      return multi(responses);
    }
    const filter = children(request, ns, "filter")[0];
    if (filter && kind === "contacts") {
      const filters = children(filter, A, "prop-filter");
      if (filter.children.length !== filters.length) throw new AppError("unsupportedFilter", 403);
      if (filter.attrs.test && !["anyof", "allof"].includes(filter.attrs.test)) throw new AppError("unsupportedFilter", 403);
      for (const f of filters) {
        if (!/^[A-Z0-9-]+$/i.test(f.attrs.name ?? "")) throw new AppError("unsupportedFilter", 403);
        const absent = children(f, A, "is-not-defined"), tests = children(f, A, "text-match");
        if (absent.length ? f.children.length !== 1 : f.children.length !== tests.length) throw new AppError("unsupportedFilter", 403);
        for (const t of tests) if (t.attrs.collation && !["i;unicode-casemap", "i;ascii-casemap"].includes(t.attrs.collation) || t.attrs["match-type"] && !["contains", "equals", "starts-with", "ends-with"].includes(t.attrs["match-type"]) || t.attrs["negate-condition"] && !["yes", "no"].includes(t.attrs["negate-condition"])) throw new AppError("unsupportedFilter", 403);
      }
      rows = rows.filter(row => {
        const card = parsed(row.content, "vcard");
        const matches = filters.map(f => {
          if (!/^[A-Z0-9-]+$/i.test(f.attrs.name ?? "")) throw new AppError("unsupportedFilter", 403);
          const values = card.getAllProperties(f.attrs.name.toLowerCase()).flatMap(p => p.getValues().flat(Infinity).map(String));
          if (children(f, A, "is-not-defined").length) return !values.length;
          const tests = children(f, A, "text-match");
          if (f.children.length !== tests.length) throw new AppError("unsupportedFilter", 403);
          return tests.length ? tests.every(t => {
            if (t.attrs.collation && !["i;unicode-casemap", "i;ascii-casemap"].includes(t.attrs.collation) || t.attrs["match-type"] && !["contains", "equals", "starts-with", "ends-with"].includes(t.attrs["match-type"])) throw new AppError("unsupportedFilter", 403);
            const wanted = t.text.toLowerCase(), type = t.attrs["match-type"] ?? "contains";
            const matches = values.some(v => { const text = v.toLowerCase(); return type === "equals" ? text === wanted : type === "starts-with" ? text.startsWith(wanted) : type === "ends-with" ? text.endsWith(wanted) : text.includes(wanted); });
            return t.attrs["negate-condition"] === "yes" ? !matches : matches;
          }) : values.length > 0;
        });
        return filter.attrs.test === "anyof" ? matches.some(Boolean) : matches.every(Boolean);
      });
    }
    if (filter && kind === "calendar") {
      const root = children(filter, C, "comp-filter")[0], components = children(root, C, "comp-filter");
      if (!root || filter.children.length !== 1 || root.children.length !== components.length || root.attrs.name !== "VCALENDAR" || components.length !== 1 || !["VEVENT","VTODO"].includes(components[0].attrs.name)) throw new AppError("unsupportedFilter", 403);
      const component = components[0], range = children(component, C, "time-range")[0];
      rows=rows.filter(row=>isTask(row.content)===(component.attrs.name==="VTODO"));
      const properties=children(component,C,"prop-filter");
      if (component.children.length !== (range ? 1 : 0)+properties.length) throw new AppError("unsupportedFilter", 403);
      for(const property of properties){const name=(property.attrs.name??"").toLowerCase(),absent=children(property,C,"is-not-defined")[0],text=children(property,C,"text-match")[0];if(!name||property.children.length!==(absent?1:text?1:0)||text&&!['i;ascii-casemap','i;unicode-casemap','i;octet',undefined].includes(text.attrs.collation))throw new AppError("unsupportedFilter",403);rows=rows.filter(row=>{const root=parsed(row.content,"vcalendar"),values=root.getAllSubcomponents(component.attrs.name.toLowerCase()).flatMap(c=>c.getAllProperties(name).flatMap(p=>p.getValues().map(String)));if(absent)return !values.length;if(!text)return !!values.length;const fold=(v:string)=>text.attrs.collation==="i;octet"?v:v.toLowerCase(),matched=values.some(v=>fold(v).includes(fold(text.text)));return values.length>0&&(text.attrs["negate-condition"]==="yes"?!matched:matched);});}
      if (range) {
        if (!range.attrs.start && !range.attrs.end) throw new AppError("invalidTimeRange");
        const from = range.attrs.start ? davDate(range.attrs.start) : new Date("0001-01-01T00:00:00Z"), to = range.attrs.end ? davDate(range.attrs.end) : new Date("9999-12-31T23:59:59Z");
        rows = rows.filter(row => isTask(row.content)?taskMatchesRange(row,from,to):expand(row, "UTC", from, to, true, true).length > 0);
      }
    }
    return multi(rows.map(row => response(path + row.href, props(row.href, false, row))));
  }
  if (!collection || !href) throw new AppError("methodNotAllowed", 405);
  if (method === "GET" || method === "HEAD") {
    if (!resource) throw new AppError("notFound", 404);
    res.writeHead(200, { "etag": pimEtag(resource), "content-type": kind === "contacts" ? "text/vcard; charset=utf-8" : "text/calendar; charset=utf-8", "content-length": Buffer.byteLength(resource.content) }); res.end(method === "HEAD" ? undefined : resource.content); return;
  }
  if (method === "PUT") {
    const revision = requireMatch(req, resource), content = await readBody(req);
    const row = await service.put(collection, href, { content }, revision, resource?.content); res.writeHead(resource ? 204 : 201, { etag: pimEtag(row) }); res.end(); return;
  }
  if (method === "DELETE") {
    if (!resource) throw new AppError("notFound", 404);
    await service.call("delete", { collectionId: collection.id, href, revision: requireMatch(req, resource) }); res.writeHead(204); res.end(); return;
  }
  throw new AppError("methodNotAllowed", 405);
}
