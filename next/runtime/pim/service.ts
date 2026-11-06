import { randomUUID } from "node:crypto";
import type { Database } from "../database.ts";
import type { PimCollection, PimKind, PimResource } from "../../shared/pim.ts";
import { readContact, writeContact } from "./contacts.ts";
import { cancelOccurrence, expand, readEvent, splitCalendarImport, writeEvent } from "./calendar.ts";
import { field, invalid, resources, serialize } from "./standards.ts";
import ICAL from "ical.js";

export const pimEtag = (row: { id: string; revision: number }) => `"${row.id}:${row.revision}"`;
export function pimService(db: Database, sessionId: string) {
  const call = <T>(operation: string, payload: object = {}) => db.call<T>("service_pim", [sessionId, operation, JSON.stringify(payload)]);
  const collections = () => call<PimCollection[]>("collections");
  const list = (collectionId: string) => call<{ collection: PimCollection; items: PimResource[] }>("list", { collectionId });
  const read = (collectionId: string, href: string) => call<PimResource>("read", { collectionId, href });
  async function put(collection: PimCollection, href: string, input: Record<string, unknown>, revision: number, previous?: string) {
    const result = collection.kind === "contacts" ? (typeof input.content === "string" ? readContact(input.content) : writeContact(input.data, previous))
      : input.cancelOccurrence ? cancelOccurrence(previous ?? "", field(input.cancelOccurrence))
      : typeof input.content === "string" ? readEvent(input.content) : writeEvent(input.data, previous, input.occurrenceId ? field(input.occurrenceId) : undefined);
    if (previous && result.uid !== (collection.kind === "contacts" ? readContact(previous).uid : readEvent(previous).uid)) invalid("A resource UID cannot change; create another resource instead.");
    return call<PimResource>("put", { collectionId: collection.id, href, revision, id: randomUUID(), uid: result.uid, content: result.content, projection: JSON.stringify(result.data) });
  }
  async function importText(collection: PimCollection, text: string) {
    const input = resources(text, collection.kind === "contacts" ? "vcard" : "vcalendar");
    const parts = collection.kind === "contacts" ? input : input.flatMap(splitCalendarImport);
    const results: { index: number; href?: string; error?: string }[] = [];
    for (const [index, content] of parts.entries()) {
      try { const href = `${randomUUID()}.${collection.kind === "contacts" ? "vcf" : "ics"}`; await put(collection, href, { content }, 0); results.push({ index, href }); }
      catch (error) { results.push({ index, error: error instanceof Error ? error.message : "Import failed." }); }
    }
    return { items: results, imported: results.filter(r => r.href).length };
  }
  async function exportText(collectionId: string) {
    const { collection, items } = await list(collectionId);
    if (items.reduce((sum, row) => sum + Buffer.byteLength(row.content), 0) > 8 * 1024 * 1024) invalid("Export exceeds 8 MiB; export individual resources or use DAV.");
    if (collection.kind === "contacts") return items.map(row => row.content).join("");
    const root = new ICAL.Component("vcalendar"); root.updatePropertyWithValue("version", "2.0"); root.updatePropertyWithValue("prodid", "-//Dar//Calendar//EN");
    const zones = new Map<string, string>();
    for (const row of items) for (const component of readEvent(row.content).root.getAllSubcomponents()) {
      if (component.name === "vtimezone") {
        const id = String(component.getFirstPropertyValue("tzid")), previous = zones.get(id), data = serialize(component);
        if (previous && previous !== data) invalid("Conflicting timezone definitions; export these events separately.");
        if (previous) continue; zones.set(id, data);
      }
      root.addSubcomponent(component);
    }
    return serialize(root);
  }
  async function occurrences(collectionId: string, zone: string, from: Date, to: Date, includeCancelled = false) {
    const { collection, items } = await list(collectionId); if (collection.kind !== "calendar") invalid("This is not a calendar.");
    const result = items.flatMap(row => expand(row, zone, from, to, false, includeCancelled));
    if (result.length > 5000) invalid("Calendar window contains too many occurrences.");
    return result.sort((a,b) => a.startAt.localeCompare(b.startAt));
  }
  return { call, collections, list, read, put, importText, exportText, occurrences };
}
export type PimService = ReturnType<typeof pimService>;
export function kindFromPath(value: string): PimKind { if (value === "contacts") return "contacts"; if (value === "calendar") return "calendar"; invalid("Unknown app."); }
