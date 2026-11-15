import assert from "node:assert/strict";
import test from "node:test";
import {localDate,localTime,localDateTime} from "../client/dates.ts";

test("cached date formatting matches the Date methods", () => {
  const options:Intl.DateTimeFormatOptions[] = [{}, {month: "long", year: "numeric"}, {dateStyle: "medium"}, {dateStyle: "medium", timeStyle: "short"},
    {hour: "numeric", minute: "2-digit"}, {month: "short", day: "numeric"}, {weekday: "long"}, {weekday: "short", hour: "numeric", minute: "2-digit"},
    {month: "long", timeZone: "UTC"}, {weekday: "long", month: "short", day: "numeric", timeZone: "UTC"}, {hour: "numeric", minute: "2-digit", timeZoneName: "short"}, {timeZone: "UTC"}];
  for (const time of [0, Date.UTC(2026, 10, 14, 18, 5, 9), Date.UTC(1999, 0, 1, 0, 0, 0)]) {
    const date = new Date(time);
    for (const value of options) {
      // Combinations the Date methods reject are not used.
      const same = (cached: () => string, native: () => string) => { let expected: string; try { expected = native(); } catch { return; } assert.equal(cached(), expected, JSON.stringify(value)); };
      same(() => localDate(time, value), () => date.toLocaleDateString(undefined, value));
      same(() => localTime(time, value), () => date.toLocaleTimeString(undefined, value));
      same(() => localDateTime(time, value), () => date.toLocaleString(undefined, value));
    }
  }
});
