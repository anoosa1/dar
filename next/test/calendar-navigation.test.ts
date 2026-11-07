import {test} from "node:test";
import assert from "node:assert/strict";
import {shiftCalendarDate} from "../shared/calendar-navigation.ts";

test("Calendar navigation follows the selected period and clamps month/year boundaries",()=>{
  for(const [date,view,direction,expected] of [
    ["2026-11-07","day",1,"2026-11-08"],["2026-11-07","day",-1,"2026-11-06"],
    ["2026-12-31","day",1,"2027-01-01"],["2026-03-07","week",1,"2026-03-14"],
    ["2026-01-31","month",1,"2026-02-28"],["2028-01-31","month",1,"2028-02-29"],
    ["2026-01-07","month",-1,"2025-12-07"],["2026-12-07","month",1,"2027-01-07"],
    ["2026-11-07","agenda",1,"2026-12-07"],
  ] as const)assert.equal(shiftCalendarDate(date,view,direction),expected);
});
