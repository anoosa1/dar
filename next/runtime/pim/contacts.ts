import { randomUUID } from "node:crypto";
import ICAL from "ical.js";
import type { ContactData, ContactValue } from "../../shared/pim.ts";
import { escapeText, field, invalid, parsed, serialize } from "./standards.ts";

const text = (c: ICAL.Component, name: string) => String(c.getFirstPropertyValue(name) ?? "");
function values(c: ICAL.Component, name: string): ContactValue[] {
  return c.getAllProperties(name).map(p => ({ value: String(p.getFirstValue()), label: String(p.getParameter("type") ?? "other").replaceAll(",", ", ") }));
}
export function readContact(content: string) {
  const c = parsed(content, "vcard"), version = text(c, "version");
  if (!["3.0", "4.0"].includes(version)) invalid("Only vCard 3.0 and 4.0 are supported.");
  if (!text(c, "uid")) c.updatePropertyWithValue("uid", randomUUID());
  if (!text(c, "fn").trim()) invalid("A contact needs a display name.");
  const names = c.getFirstPropertyValue("n") as string[] | undefined;
  const data: ContactData = {
    displayName: text(c, "fn"), lastName: String(names?.[0] ?? ""), firstName: String(names?.[1] ?? ""), middleName: String(names?.[2] ?? ""), prefix: String(names?.[3] ?? ""), suffix: String(names?.[4] ?? ""),
    organization: [c.getFirstPropertyValue("org")].flat().filter(Boolean).join(" · "), title: text(c, "title"), notes: text(c, "note"), birthday: text(c, "bday"),
    emails: values(c, "email"), phones: values(c, "tel"), urls: values(c, "url"),
    addresses: c.getAllProperties("adr").map(p => { const a = p.getFirstValue() as string[]; return { label: String(p.getParameter("type") ?? "home"), poBox: String(a[0] ?? ""), extended: String(a[1] ?? ""), street: String(a[2] ?? ""), city: String(a[3] ?? ""), region: String(a[4] ?? ""), postalCode: String(a[5] ?? ""), country: String(a[6] ?? "") }; }),
    groups: c.getAllProperties("categories").flatMap(p => p.getValues().map(String)), favorite: text(c, "x-dar-favorite") === "1",
  };
  return { uid: text(c, "uid"), content: serialize(c), data };
}
export function writeContact(value: unknown, previous?: string) {
  if (!value || typeof value !== "object") invalid("Invalid contact.");
  const p = value as Record<string, unknown>;
  const previousData = previous ? readContact(previous) : undefined;
  const strings = ["displayName", "firstName", "lastName", "middleName", "prefix", "suffix", "organization", "title", "notes", "birthday"] as const;
  const data = Object.fromEntries(strings.map(name => [name, field(p[name] ?? "", name === "notes" ? 16384 : 1024)])) as Pick<ContactData, typeof strings[number]>;
  data.displayName = data.displayName.trim() || `${data.firstName} ${data.lastName}`.trim();
  if (!data.displayName) invalid("A contact needs a name.");
  const unchangedBirthday = !!previousData && data.birthday === previousData.data.birthday;
  if (data.birthday && !unchangedBirthday && !/^(\d{4}-\d{2}-\d{2}|--\d{2}-\d{2})$/.test(data.birthday)) invalid("Birthday must be YYYY-MM-DD or --MM-DD.");
  if (data.birthday && !unchangedBirthday) { const date = data.birthday.startsWith("--") ? "2000" + data.birthday.slice(1) : data.birthday; const parsed = new Date(`${date}T00:00:00Z`); if (!Number.isFinite(+parsed) || parsed.toISOString().slice(0,10) !== date) invalid("Invalid birthday date."); }
  const lines = ["BEGIN:VCARD", "VERSION:3.0", `UID:${escapeText(previousData?.uid ?? randomUUID())}`, `FN:${escapeText(data.displayName)}`, `N:${[data.lastName, data.firstName, data.middleName, data.prefix, data.suffix].map(escapeText).join(";")}`];
  for (const [name, property] of [["emails", "EMAIL"], ["phones", "TEL"], ["urls", "URL"]] as const) {
    const items = p[name] ?? []; if (!Array.isArray(items) || items.length > 30) invalid("Too many contact values.");
    for (const item of items) {
      if (!item || typeof item !== "object") invalid("Invalid contact value.");
      const v = field(item.value, 2048), label = field(item.label ?? "other", 50);
      if (!/^[a-zA-Z0-9 _,\-]+$/.test(label)) invalid("Invalid contact label.");
      if (v.trim()) lines.push(`${property};TYPE=${label.split(/[,\s]+/).filter(Boolean).join(",")}:${escapeText(v)}`);
    }
  }
  const addresses = p.addresses ?? []; if (!Array.isArray(addresses) || addresses.length > 20) invalid("Too many addresses.");
  for (const address of addresses) {
    if (!address || typeof address !== "object") invalid("Invalid postal address.");
    const label = field(address.label ?? "home", 50); if (!/^[a-zA-Z0-9_-]+$/.test(label)) invalid("Invalid address label.");
    lines.push(`ADR;TYPE=${label}:${["poBox", "extended", "street", "city", "region", "postalCode", "country"].map(key => escapeText(field(address[key] ?? "", 1024))).join(";")}`);
  }
  for (const [name, property] of [["organization", "ORG"], ["title", "TITLE"], ["notes", "NOTE"], ["birthday", "BDAY"]] as const) if (data[name] && !(name === "birthday" && unchangedBirthday)) lines.push(`${property}:${escapeText(data[name])}`);
  const rawGroups = p.groups ?? []; if (!Array.isArray(rawGroups) || rawGroups.length > 50) invalid("Too many groups.");
  const groups = [...new Set(rawGroups.map(v => field(v, 100).trim()).filter(Boolean))];
  if (groups.length) lines.push(`CATEGORIES:${groups.map(v => escapeText(field(v, 100))).join(",")}`);
  if (p.favorite === true) lines.push("X-DAR-FAVORITE:1");
  lines.push("END:VCARD");
  const updated = parsed(lines.join("\r\n"), "vcard");
  // The vCard 3 and 4 design sets differ in structured-text escaping. Set
  // unstructured values through the library instead of reparsing escaped text.
  updated.updatePropertyWithValue("fn", data.displayName);
  for (const [property, value] of [["title", data.title], ["note", data.notes]] as const) if (value) updated.updatePropertyWithValue(property, value);
  if (data.organization) updated.updatePropertyWithValue("org", [data.organization]);
  if (!previous) return readContact(serialize(updated));
  const old = parsed(previous, "vcard");
  const managed = ["uid", "fn", "n", "email", "tel", "url", "adr", "org", "title", "note", "bday", "categories", "x-dar-favorite"];
  for (const name of managed) {
    if (name === "bday" && unchangedBirthday || name === "org" && data.organization === previousData?.data.organization || name === "n" && ["lastName","firstName","middleName","prefix","suffix"].every(key => data[key as keyof typeof data] === previousData?.data[key as keyof ContactData])) continue;
    const previousProperties = old.getAllProperties(name);
    old.removeAllProperties(name);
    for (const prop of updated.getAllProperties(name)) {
      // Retain parameters (PREF, PID, language, custom labels, etc.) on values
      // untouched by an edit to another field.
      const same = previousProperties.find(p => JSON.stringify(p.getValues()) === JSON.stringify(prop.getValues()) && JSON.stringify(p.getParameter("type") ?? null) === JSON.stringify(prop.getParameter("type") ?? null));
      old.addProperty(same ?? prop);
    }
  }
  return readContact(serialize(old));
}
