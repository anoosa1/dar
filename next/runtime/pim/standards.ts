import ICAL from "ical.js";
import { AppError } from "../errors.ts";

export function invalid(message: string): never { throw new AppError("invalidInput", 400, message); }
export function field(value: unknown, max = 4096): string {
  if (typeof value !== "string" || value.length > max || /\u0000/.test(value)) invalid("Invalid text field.");
  return value.replace(/\r\n?/g, "\n");
}
export function parsed(text: string, expected: string): ICAL.Component {
  if (Buffer.byteLength(text) > 262144 || !text.trim()) invalid("Resource is empty or too large.");
  try {
    const raw = ICAL.parse(text);
    if (raw[0] !== expected) invalid(`Expected ${expected}.`);
    return new ICAL.Component(raw);
  } catch (error) { if (error instanceof AppError) throw error; invalid("The standards resource could not be parsed."); }
}
export const escapeText = (value: string) => value.replaceAll("\\", "\\\\").replaceAll("\r\n", "\n").replaceAll("\n", "\\n").replaceAll(";", "\\;").replaceAll(",", "\\,");
export function serialize(component: ICAL.Component): string {
  // Fold by UTF-8 octets, preserving Unicode scalars and existing escaped values.
  return component.toString().replace(/\r?\n[ \t]/g, "").split(/\r?\n/).filter(Boolean).map(line => {
    const pieces = []; let current = "", bytes = 0;
    for (const scalar of line) { const n = Buffer.byteLength(scalar); if (bytes + n > 75) { pieces.push(current); current = " "; bytes = 1; } current += scalar; bytes += n; }
    pieces.push(current); return pieces.join("\r\n");
  }).join("\r\n") + "\r\n";
}
export function resources(text: string, kind: "vcard" | "vcalendar"): string[] {
  if (Buffer.byteLength(text) > 2 * 1024 * 1024) invalid("Import exceeds 2 MiB.");
  const begin = `BEGIN:${kind.toUpperCase()}`, end = `END:${kind.toUpperCase()}`;
  const result: string[] = []; let current: string[] | undefined;
  for (const line of text.replace(/^\uFEFF/, "").split(/\r?\n/)) {
    if (line.toUpperCase() === begin) { if (current) invalid("Nested resources are not valid."); current = [line]; }
    else if (line.toUpperCase() === end) { if (!current) invalid("Unexpected resource ending."); current.push(line); result.push(current.join("\r\n")); current = undefined; }
    else if (current) current.push(line);
    else if (line.trim()) invalid("Text outside a standards resource.");
    if (result.length > 1000) invalid("Import exceeds 1,000 resources.");
  }
  if (current || !result.length) invalid("Incomplete or empty import.");
  return result;
}
