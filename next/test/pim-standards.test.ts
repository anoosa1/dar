import { test } from "node:test";
import assert from "node:assert/strict";
import { readContact, writeContact } from "../runtime/pim/contacts.ts";
import { writeEvent, readEvent, expand, wallEpoch, splitCalendarImport, cancelOccurrence } from "../runtime/pim/calendar.ts";
import type { PimResource } from "../shared/pim.ts";

test("contact editing preserves unknown vCard properties, UID and multi-value data", () => {
  for (const version of ["3.0", "4.0"]) {
    const source = `BEGIN:VCARD\r\nVERSION:${version}\r\nUID:contact-1\r\nFN:Amira Lee\r\nN:Lee;Amira;;;\r\nEMAIL;TYPE=work,voice;X-CUSTOM=keep:amira@example.com\r\nEMAIL;TYPE=home:home@example.com\r\nX-EXTERNAL-PROPERTY:Keep me\r\nEND:VCARD\r\n`;
    const original = readContact(source), changed = writeContact({ ...original.data, title: "Designer", groups: ["Friends", "Design, team"], notes: "a; b, c\nand another line 🔐" }, source);
    assert.equal(changed.uid, "contact-1"); assert.equal(changed.data.emails.length, 2);
    assert.equal(changed.data.notes, "a; b, c\nand another line 🔐"); assert.deepEqual(changed.data.groups, ["Friends", "Design, team"]);
    assert.match(changed.content, /X-EXTERNAL-PROPERTY:Keep me/); assert.match(changed.content, new RegExp(`VERSION:${version}`));
    assert.match(changed.content, /X-CUSTOM=keep/);
    for (const line of changed.content.split("\r\n")) assert.ok(Buffer.byteLength(line) <= 75);
  }
  assert.throws(() => writeContact({ displayName: "" })); assert.throws(() => readContact("BEGIN:VCARD\r\nVERSION:4.0\r\nEND:VCARD"));
  const complex = "BEGIN:VCARD\r\nVERSION:4.0\r\nUID:complex\r\nFN:Amira Lee\r\nN:Lee;Amira,Samira;;;\r\nORG:Northstar;Design\r\nBDAY;VALUE=text:circa 1990\r\nADR;TYPE=home:42;Building B;Street;City;Region;12345;Country\r\nEND:VCARD\r\n";
  const before = readContact(complex), after = writeContact({ ...before.data, notes: "Only notes changed" },complex);
  assert.match(after.content,/ORG:Northstar;Design/); assert.match(after.content,/N:Lee;Amira,Samira;;;/); assert.match(after.content,/BDAY;VALUE=text:circa 1990/i);
  assert.equal(after.data.addresses[0].poBox,"42"); assert.equal(after.data.addresses[0].extended,"Building B");
});
const base = { title: "Design review", start: "2026-03-01T09:00", end: "2026-03-01T10:00", timezone: "America/Toronto", recurrence: "FREQ=WEEKLY;COUNT=4", attendees: [], reminderMinutes: 10, allDay: false };
function resource(content: string) { return { id: "r", collectionId: "c", href: "e.ics", revision: 1, content } as PimResource; }
test("weekly local-time recurrence crosses DST without shifting the wall time", () => {
  const event = writeEvent(base), list = expand(resource(event.content), "UTC", new Date("2026-03-01"), new Date("2026-04-01"));
  assert.equal(list.length, 4); assert.equal(list[0].startAt, "2026-03-01T14:00:00.000Z"); assert.equal(list[1].startAt, "2026-03-08T13:00:00.000Z");
  assert.equal(list[0].reminderMinutes, 10);
  const earlier = writeEvent({ ...base, start:"2010-07-01T09:00", end:"2010-07-01T10:00", recurrence:"" },event.content);
  assert.equal(expand(resource(earlier.content),"UTC",new Date("2010-07-01"),new Date("2010-07-02"))[0].startAt,"2010-07-01T13:00:00.000Z");
  const utc = writeEvent({ ...base, timezone: "UTC", recurrence: "" });
  assert.match(utc.content, /DTSTART:20260301T090000Z/); assert.equal(utc.data.timezone, "UTC");
  assert.equal(expand(resource(utc.content), "America/Toronto", new Date("2026-03-01"), new Date("2026-03-02"))[0].startAt, "2026-03-01T09:00:00.000Z");
  assert.throws(() => writeEvent({ ...base, start: "2026-03-08T02:30" }), /does not exist/);
  assert.equal(new Date(wallEpoch("2026-11-01T01:30", "America/Toronto")).toISOString(), "2026-11-01T05:30:00.000Z");
});
test("all-day intervals have exclusive ends across DST, and floating events follow viewer timezone", () => {
  const day = writeEvent({ ...base, allDay: true, start: "2026-03-08", end: "2026-03-09", recurrence: "" });
  const occurrence = expand(resource(day.content), "America/Toronto", new Date("2026-03-07"), new Date("2026-03-10"))[0];
  assert.equal(Date.parse(occurrence.endAt)-Date.parse(occurrence.startAt), 23*3600000);
  const floating = writeEvent({ ...base, timezone: "floating", recurrence: "" });
  assert.equal(expand(resource(floating.content), "America/Toronto", new Date("2026-03-01"), new Date("2026-03-02"))[0].startAt, "2026-03-01T14:00:00.000Z");
});
test("recurrence exceptions, EXDATE/RDATE, cancellations and unknown fields survive edit", () => {
  const event = writeEvent(base), raw = event.content.replace("END:VEVENT", "EXDATE;TZID=America/Toronto:20260315T090000\r\nRDATE;TZID=America/Toronto:20260330T090000\r\nX-EXTERNAL:preserved\r\nEND:VEVENT");
  const changed = writeEvent({ ...base, start: "2026-03-08T11:00", end: "2026-03-08T12:00", title: "Moved review" }, raw, "2026-03-08T09:00:00");
  assert.match(changed.content, /X-EXTERNAL:preserved/);
  let list = expand(resource(changed.content), "America/Toronto", new Date("2026-03-01"), new Date("2026-04-01"));
  assert.equal(list.length, 4); assert.equal(list.find(x => x.title === "Moved review")?.startAt, "2026-03-08T15:00:00.000Z");
  const cancelled = cancelOccurrence(changed.content, "2026-03-22T09:00:00");
  list = expand(resource(cancelled.content), "UTC", new Date("2026-03-01"), new Date("2026-04-01")); assert.equal(list.length, 3);
});
test("leap-day recurrence and bounded infinite series, multi-event imports", () => {
  const leap = writeEvent({ ...base, allDay: true, start: "2024-02-29", end: "2024-03-01", recurrence: "FREQ=YEARLY;COUNT=3" });
  assert.equal(expand(resource(leap.content), "UTC", new Date("2028-01-01"), new Date("2029-01-01"))[0].start, "2028-02-29");
  const dense = writeEvent({ ...base, recurrence: "FREQ=SECONDLY" });
  assert.throws(() => expand(resource(dense.content), "UTC", new Date("2026-03-01"), new Date("2026-03-02")), /Too many|limit/);
  const first = writeEvent({ ...base, recurrence: "" }), second = writeEvent({ ...base, title: "Second", recurrence: "" });
  const combined = first.content.replace("END:VCALENDAR", second.content.match(/BEGIN:VEVENT[\s\S]*END:VEVENT/)![0] + "\r\nEND:VCALENDAR");
  assert.equal(splitCalendarImport(combined).length, 2); assert.throws(() => readEvent(combined), /exactly one/);
});
test("RDATE periods keep their own duration and native zoned exports include VTIMEZONE", () => {
  const event = writeEvent({ ...base, recurrence: "" }); assert.match(event.content, /BEGIN:VTIMEZONE/);
  const raw = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:period\r\nSUMMARY:Period\r\nDTSTART:20261109T090000Z\r\nDURATION:PT1H\r\nRDATE;VALUE=PERIOD:20261110T090000Z/PT3H\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
  const list = expand(resource(raw), "UTC", new Date("2026-11-09"), new Date("2026-11-11"));
  assert.equal(list.length,2); assert.equal(list[1].endAt,"2026-11-10T12:00:00.000Z");
});
test("an exception moved years before its original slot is still visible in its new window", () => {
  const series = writeEvent({ ...base, timezone:"UTC", start:"2030-03-01T09:00", end:"2030-03-01T10:00", recurrence:"FREQ=YEARLY;COUNT=5" });
  const moved = writeEvent({ ...base, timezone:"UTC", start:"2026-03-01T11:00", end:"2026-03-01T12:00", title:"Moved from 2032" }, series.content, "2032-03-01T09:00:00Z");
  const list = expand(resource(moved.content),"UTC",new Date("2026-03-01"),new Date("2026-03-02"));
  assert.equal(list.length,1); assert.equal(list[0].title,"Moved from 2032");
});
