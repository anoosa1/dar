import ICAL from "ical.js";
import { randomUUID } from "node:crypto";
import type { EventData, Occurrence, PimResource } from "../../shared/pim.ts";
import { AppError } from "../errors.ts";
import { escapeText, field, invalid, parsed, serialize } from "./standards.ts";
import { timezoneComponent } from "./timezones.ts";

const validZones = new Set<string>();
const formatters = new Map<string, Intl.DateTimeFormat>();
export function timezone(value: string): string {
  if (validZones.has(value)) return value;
  try { new Intl.DateTimeFormat("en", { timeZone: value }).format(); if (validZones.size < 1024) validZones.add(value); return value; }
  catch { invalid("Choose a valid IANA timezone."); }
}
export function wall(epoch: number, zone: string): string {
  let formatter = formatters.get(zone);
  if (!formatter) { formatter = new Intl.DateTimeFormat("en-CA", { timeZone: zone, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" }); if (formatters.size < 1024) formatters.set(zone, formatter); }
  const parts = new Map(formatter.formatToParts(epoch).map(p => [p.type, p.value]));
  return `${parts.get("year")}-${parts.get("month")}-${parts.get("day")}T${parts.get("hour")}:${parts.get("minute")}:${parts.get("second")}`;
}
export function wallEpoch(value: string, zone: string, rejectGap = false): number {
  timezone(zone);
  const local = value.length === 10 ? `${value}T00:00:00` : value.length === 16 ? `${value}:00` : value;
  if (!/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d$/.test(local)) invalid("Invalid date and time.");
  const naive = Date.parse(`${local}Z`);
  if (!Number.isFinite(naive) || new Date(naive).toISOString().slice(0, 19) !== local) invalid("Invalid calendar date.");
  const probes = [-48, 0, 48].map(hours => naive + hours * 3600000);
  const offsets = probes.map(probe => Date.parse(`${wall(probe, zone)}Z`) - probe);
  const matches = [...new Set(offsets)].map(offset => naive - offset).filter(epoch => wall(epoch, zone) === local).sort((a,b) => a-b);
  if (matches.length) return matches[0];
  if (rejectGap) invalid("That local time does not exist because the clocks change.");
  // RFC 5545: the offset before a spring-forward gap resolves imported values.
  return naive - offsets[0];
}
function value(c: ICAL.Component, name: string) { return String(c.getFirstPropertyValue(name) ?? ""); }
function timeZone(c: ICAL.Component, property = "dtstart") {
  const t = c.getFirstPropertyValue(property) as ICAL.Time | null;
  return t?.isDate ? "floating" : String(c.getFirstProperty(property)?.getParameter("tzid") ?? (t?.zone.tzid === "UTC" ? "UTC" : "floating"));
}
function instant(t: ICAL.Time, c: ICAL.Component, viewerZone: string, property = "dtstart") {
  const zone = timeZone(c, c.hasProperty(property) ? property : "dtstart");
  if (!t.isDate && t.zone.tzid !== "floating" && t.zone.tzid !== "local") return t.toUnixTime() * 1000;
  return wallEpoch(t.toString(), zone === "floating" ? viewerZone : zone);
}
function dateString(t: ICAL.Time) { return t.toString().replace(/Z$/, ""); }
function projection(c: ICAL.Component): EventData {
  const e = new ICAL.Event(c), alarm = c.getAllSubcomponents("valarm").find(a => value(a, "action") === "DISPLAY"), trigger = alarm?.getFirstPropertyValue("trigger");
  return { title: value(c, "summary"), start: dateString(e.startDate), end: dateString(e.endDate), timezone: timeZone(c), allDay: e.startDate.isDate,
    description: value(c, "description"), location: value(c, "location"), recurrence: String(c.getFirstPropertyValue("rrule") ?? ""), recurring: c.hasProperty("rrule") || c.hasProperty("rdate"),
    attendees: c.getAllProperties("attendee").map(p => String(p.getFirstValue()).replace(/^mailto:/i, "")),
    reminderMinutes: trigger instanceof ICAL.Duration && alarm?.getFirstProperty("trigger")?.getParameter("related") !== "END" ? -trigger.toSeconds() / 60 : null,
    status: value(c, "status") || "CONFIRMED" };
}
export function readEvent(content: string) {
  const root = parsed(content, "vcalendar"), components = root.getAllSubcomponents("vevent");
  if (value(root, "version") !== "2.0" || !components.length || root.getAllSubcomponents().some(c => !["vevent", "vtimezone"].includes(c.name))) invalid("Expected a vCalendar 2.0 event resource.");
  const masters = components.filter(c => !c.hasProperty("recurrence-id"));
  if (masters.length !== 1) invalid("An event resource needs exactly one master event.");
  const master = masters[0], uid = value(master, "uid");
  if (!uid || uid.length > 1024) invalid("An event needs a stable UID.");
  const recurrenceIds = new Set<string>();
  for (const c of components) {
    if (value(c, "uid") !== uid || !c.hasProperty("dtstart")) invalid("Event components must share a UID and have a start.");
    const e = new ICAL.Event(c);
    if (!e.startDate || !e.endDate || e.startDate.isDate !== e.endDate.isDate || e.endDate.compare(e.startDate) < 0) invalid("Invalid event interval.");
    const zone = timeZone(c);
    if (zone !== "floating" && !root.getAllSubcomponents("vtimezone").some(t => value(t, "tzid") === zone)) timezone(zone);
    for (const property of ["dtstart", "dtend"]) {
      const t = c.getFirstPropertyValue(property) as ICAL.Time | null;
      if (t) instant(t, c, "UTC", property);
    }
    if (c.hasProperty("recurrence-id")) { const key = value(c, "recurrence-id"); if (recurrenceIds.has(key)) invalid("Duplicate recurrence exception."); recurrenceIds.add(key); }
    if (!["", "CONFIRMED", "TENTATIVE", "CANCELLED"].includes(value(c, "status"))) invalid("Invalid event status.");
    for (const alarm of c.getAllSubcomponents("valarm")) {
      if (!["DISPLAY", "AUDIO", "EMAIL"].includes(value(alarm, "action"))) invalid("Invalid alarm action.");
    }
  }
  return { uid, content: serialize(root), data: projection(master), root, master };
}
function localTime(value: string, allDay: boolean, zone: string): ICAL.Time {
  if (allDay) { wallEpoch(value, "UTC", true); return ICAL.Time.fromDateString(value); }
  if (zone !== "floating") wallEpoch(value, zone, true); else wallEpoch(value, "UTC", true);
  return ICAL.Time.fromDateTimeString(value.length === 16 ? `${value}:00` : value);
}
function setTime(c: ICAL.Component, name: string, time: ICAL.Time, zone: string) {
  // setValue snapshots the serialized value, so assign UTC before calling it.
  if (!time.isDate) time.zone = zone === "UTC" ? ICAL.Timezone.utcTimezone : ICAL.Timezone.localTimezone;
  c.removeAllProperties(name); const p = new ICAL.Property(name); p.setValue(time);
  if (!time.isDate && zone !== "floating") {
    if (zone !== "UTC") p.setParameter("tzid", zone);
  }
  c.addProperty(p);
}
export function writeEvent(input: unknown, previous?: string, occurrenceId?: string) {
  if (!input || typeof input !== "object") invalid("Invalid event.");
  const p = input as Record<string, unknown>, zone = field(p.timezone ?? "UTC", 100);
  if (zone !== "floating") timezone(zone);
  const allDay = p.allDay === true, start = localTime(field(p.start), allDay, zone), end = localTime(field(p.end), allDay, zone);
  const compareZone = zone === "floating" ? "UTC" : zone;
  if (wallEpoch(dateString(end), compareZone) <= wallEpoch(dateString(start), compareZone)) invalid("End must be after start.");
  const previousEvent = previous ? readEvent(previous) : undefined;
  const root = previousEvent?.root ?? parsed(`BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Dar//Calendar//EN\r\nEND:VCALENDAR\r\n`, "vcalendar");
  let c = previousEvent?.master ?? new ICAL.Component("vevent");
  const uid = previousEvent?.uid ?? randomUUID();
  if (!previousEvent) { c.updatePropertyWithValue("uid", uid); root.addSubcomponent(c); }
  if (occurrenceId) {
    if (!previousEvent) invalid("An occurrence needs an existing series.");
    const key = ICAL.Time.fromString(occurrenceId, undefined);
    const found = root.getAllSubcomponents("vevent").find(item => value(item, "recurrence-id") === key.toString());
    c = found ?? new ICAL.Component(JSON.parse(JSON.stringify(previousEvent.master.toJSON())));
    if (!found) root.addSubcomponent(c);
    for (const name of ["rrule", "rdate", "exdate"]) c.removeAllProperties(name);
    setTime(c, "recurrence-id", key, timeZone(previousEvent.master));
  }
  const title = field(p.title, 1024).trim(); if (!title) invalid("An event needs a title.");
  for (const [name, data] of [["summary", title], ["description", field(p.description ?? "", 16384)], ["location", field(p.location ?? "", 2048)], ["status", field(p.status ?? "CONFIRMED", 20)]] as const) c.updatePropertyWithValue(name, data);
  setTime(c, "dtstart", start, zone); c.removeAllProperties("duration"); setTime(c, "dtend", end, zone);
  if (!allDay && zone !== "UTC" && zone !== "floating") {
    const definition = root.getAllSubcomponents("vtimezone").find(component => value(component, "tzid") === zone);
    const firstYear = (definition?.getFirstSubcomponent("standard")?.getFirstPropertyValue("dtstart") as ICAL.Time | null)?.year;
    if (!definition || value(definition, "x-dar-generated") === "1" && firstYear !== undefined && firstYear > start.year) {
      if (definition) root.removeSubcomponent(definition);
      root.addSubcomponent(timezoneComponent(zone, start.year));
    }
  }
  if (!occurrenceId) {
    const recurrence = field(p.recurrence ?? "", 2048);
    c.removeAllProperties("rrule");
    if (recurrence) { try { c.addPropertyWithValue("rrule", ICAL.Recur.fromString(recurrence)); } catch { invalid("Invalid recurrence rule."); } }
  }
  const attendees = p.attendees ?? []; if (!Array.isArray(attendees) || attendees.length > 100) invalid("Too many attendees.");
  const oldAttendees = new Map(c.getAllProperties("attendee").map(a => [String(a.getFirstValue()).toLowerCase(), a]));
  c.removeAllProperties("attendee");
  for (const attendee of attendees) {
    const email = field(attendee, 320); if (!/^[^\s@]+@[^\s@]+$/.test(email) || /[\r\n]/.test(email)) invalid("Invalid attendee email.");
    const uri = `mailto:${email}`; c.addProperty(oldAttendees.get(uri.toLowerCase()) ?? new ICAL.Property(ICAL.parse.property(`ATTENDEE:${uri}`)));
  }
  // Preserve imported alarm metadata when the visible setting is unchanged.
  // Explicit changes replace display alarms; email/audio remain inert data.
  const minutes = p.reminderMinutes;
  const alarmChanged = !previousEvent || minutes !== projection(c).reminderMinutes;
  if (alarmChanged) for (const alarm of c.getAllSubcomponents("valarm")) if (value(alarm, "action") === "DISPLAY") c.removeSubcomponent(alarm);
  if (alarmChanged && minutes !== null && minutes !== undefined && minutes !== "") {
    if (typeof minutes !== "number" || !Number.isSafeInteger(minutes) || minutes < 0 || minutes > 10080) invalid("Reminder must be between 0 and 10,080 minutes.");
    const alarm = parsed(`BEGIN:VALARM\r\nACTION:DISPLAY\r\nTRIGGER:-PT${minutes}M\r\nDESCRIPTION:${escapeText(title)}\r\nX-DAR-MANAGED:1\r\nEND:VALARM\r\n`, "valarm"); c.addSubcomponent(alarm);
  }
  c.updatePropertyWithValue("dtstamp", ICAL.Time.fromJSDate(new Date(), true));
  c.updatePropertyWithValue("sequence", Number(c.getFirstPropertyValue("sequence") ?? 0) + 1);
  return readEvent(serialize(root));
}
export function cancelOccurrence(content: string, recurrenceId: string) {
  const event = readEvent(content);
  const occurrence = expand({ id: "", collectionId: "", href: "", revision: 0, content } as PimResource, "UTC", new Date(wallEpoch(recurrenceId.replace(/Z$/, ""), "UTC") - 2 * 86400000), new Date(wallEpoch(recurrenceId.replace(/Z$/, ""), "UTC") + 2 * 86400000)).find(o => o.recurrenceId === recurrenceId);
  if (!occurrence) invalid("Occurrence does not exist.");
  return writeEvent({ ...occurrence, status: "CANCELLED" }, event.content, recurrenceId);
}
export function expand(resource: PimResource, viewerZone: string, from: Date, to: Date, firstMatchOnly = false, includeCancelled = false): Occurrence[] {
  timezone(viewerZone);
  if (!Number.isFinite(+from) || !Number.isFinite(+to) || +to <= +from || !firstMatchOnly && +to - +from > 366 * 86400000) invalid("Calendar windows must be between one instant and one year.");
  const { root, master } = readEvent(resource.content);
  const periodEnds = new Map<string, ICAL.Time>();
  for (const property of master.getAllProperties("rdate")) {
    const values = property.getValues();
    if (!(values[0] instanceof ICAL.Period)) continue;
    const starts: ICAL.Time[] = [];
    for (const period of values as ICAL.Period[]) { const end = period.getEnd(); if (end.compare(period.start) <= 0) invalid("Invalid recurrence period."); starts.push(period.start); periodEnds.set(period.start.toString(), end); }
    const replacement = new ICAL.Property("rdate"); replacement.setValues(starts);
    const zone = property.getParameter("tzid"); if (zone) replacement.setParameter("tzid", zone);
    master.removeProperty(property); master.addProperty(replacement);
  }
  // Pin implicit month/day constraints. ical.js otherwise normalizes a Feb 29
  // yearly anniversary into March 1 instead of skipping invalid dates.
  for (const property of master.getAllProperties("rrule")) {
    const rule = property.getFirstValue() as ICAL.Recur;
    if (!["BYDAY", "BYMONTHDAY", "BYYEARDAY", "BYWEEKNO"].some(key => key in rule.parts)) {
      if (rule.freq === "YEARLY") { if (!("BYMONTH" in rule.parts)) rule.setComponent("BYMONTH", [new ICAL.Event(master).startDate.month]); rule.setComponent("BYMONTHDAY", [new ICAL.Event(master).startDate.day]); }
      else if (rule.freq === "MONTHLY") rule.setComponent("BYMONTHDAY", [new ICAL.Event(master).startDate.day]);
    }
  }
  const event = new ICAL.Event(master);
  if (master.hasProperty("rdate") && !master.hasProperty("rrule") && !master.getAllProperties("rdate").some(p => p.getValues().some((t: ICAL.Time) => t.compare(event.startDate) === 0))) {
    // DTSTART is part of the recurrence set even for an RDATE-only resource.
    const first = new ICAL.Property("rdate"); first.setValue(event.startDate);
    const zone = master.getFirstProperty("dtstart")?.getParameter("tzid"); if (zone) first.setParameter("tzid", zone);
    master.addProperty(first);
  }
  for (const c of root.getAllSubcomponents("vevent")) if (c !== master) event.relateException(c);
  const iterator = event.iterator(), result: Occurrence[] = [], seen = new Set<string>();
  for (let i = 0; ; i++) {
    if (i >= 10000) throw new AppError("operationTooLarge", 413, "This series exceeds the recurrence expansion limit.");
    const next = iterator.next(); if (!next) break;
    const key = next.toString(); if (seen.has(key)) continue; seen.add(key);
    const details = event.getOccurrenceDetails(next), component = details.item.component;
    const endDate = component === master ? periodEnds.get(next.toString()) ?? details.endDate : details.endDate;
    const startAt = instant(details.startDate, component, viewerZone), endAt = instant(endDate, component, viewerZone, "dtend");
    // Use the original recurrence position to terminate, since a moved exception
    // can appear much earlier or later than its original slot.
    const original = instant(next, master, viewerZone);
    if (original > +to + 366 * 86400000) break;
    if ((includeCancelled || value(component, "status") !== "CANCELLED") && startAt < +to && (endAt > +from || endAt === startAt && startAt >= +from)) {
      result.push({ ...projection(component), recurring: event.isRecurring(), start: dateString(details.startDate), end: dateString(endDate), resourceId: resource.id, collectionId: resource.collectionId, href: resource.href, revision: resource.revision,
        recurrenceId: next.toString(), startAt: new Date(startAt).toISOString(), endAt: new Date(endAt).toISOString() });
      if (firstMatchOnly) break;
      if (result.length > 2000) throw new AppError("operationTooLarge", 413, "Too many occurrences in this window.");
    }
    if (!event.isRecurring()) break;
    if (original > +to && !root.getAllSubcomponents("vevent").some(c => c.hasProperty("recurrence-id") && new ICAL.Event(c).startDate.compare(next) > 0)) break;
  }
  // An exception can move years away from its original recurrence slot. Check
  // explicit overrides independently of the iterator's requested-window stop.
  for (const component of root.getAllSubcomponents("vevent")) {
    if (!component.hasProperty("recurrence-id") || firstMatchOnly && result.length) continue;
    const recurrenceId = value(component, "recurrence-id");
    if (result.some(item => item.recurrenceId === recurrenceId) || !includeCancelled && value(component, "status") === "CANCELLED") continue;
    const exception = new ICAL.Event(component), startAt = instant(exception.startDate, component, viewerZone), endAt = instant(exception.endDate, component, viewerZone, "dtend");
    if (startAt < +to && (endAt > +from || endAt === startAt && startAt >= +from)) result.push({ ...projection(component), recurring: true,
      start: dateString(exception.startDate), end: dateString(exception.endDate), resourceId: resource.id, collectionId: resource.collectionId, href: resource.href, revision: resource.revision, recurrenceId,
      startAt: new Date(startAt).toISOString(), endAt: new Date(endAt).toISOString() });
    if (result.length > 2000) throw new AppError("operationTooLarge", 413, "Too many occurrences in this window.");
  }
  return result.sort((a,b) => a.startAt.localeCompare(b.startAt));
}
export function splitCalendarImport(content: string): string[] {
  const root = parsed(content, "vcalendar"), groups = new Map<string, ICAL.Component[]>();
  for (const c of root.getAllSubcomponents().filter(c=>["vevent","vtodo"].includes(c.name))) { const uid = value(c, "uid"); if (!uid) invalid("Imported resources need UIDs."); const key=`${c.name}/${uid}`,group = groups.get(key) ?? []; group.push(c); groups.set(key, group); }
  if (!groups.size || groups.size > 1000) invalid("Import needs between 1 and 1,000 event resources.");
  return [...groups.values()].map(components => {
    const copy = new ICAL.Component(JSON.parse(JSON.stringify(root.toJSON()))); copy.removeAllSubcomponents("vevent");copy.removeAllSubcomponents("vtodo");
    for (const c of components) copy.addSubcomponent(new ICAL.Component(JSON.parse(JSON.stringify(c.toJSON()))));
    return serialize(copy);
  });
}
