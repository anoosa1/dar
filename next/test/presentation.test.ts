import {test} from "node:test";
import assert from "node:assert/strict";
import {contactHref,taskDate,taskPriority,taskStatus,recurrenceLabel} from "../client/pim/presentation.ts";

test("contact actions only link supported schemes and encode email query characters",()=>{
  assert.equal(contactHref("Email","name+tag@example.com"),"mailto:name%2Btag@example.com");
  assert.equal(contactHref("Phone","+1 (416) 555-0100"),"tel:+14165550100");
  assert.equal(contactHref("Website","example.com"),"https://example.com/");
  for(const value of ["javascript:alert(1)","data:text/html,unsafe","https://user:password@example.com"])
    assert.equal(contactHref("Website",value),undefined);
});
test("task presentation keeps calendar dates, readable statuses and recurrence limits",()=>{
  assert.match(taskDate("2026-11-07",true),/7/);
  assert.equal(taskStatus["NEEDS-ACTION"],"To do");assert.equal(taskPriority(1),"High");
  assert.equal(recurrenceLabel("FREQ=WEEKLY;COUNT=4"),"Every week · 4 occurrences");
  assert.equal(recurrenceLabel("",true),"Custom schedule");
  assert.equal(recurrenceLabel("FREQ=WEEKLY;BYDAY=MO,WE"),"Custom schedule");
  assert.match(recurrenceLabel("FREQ=DAILY;UNTIL=20261107T230000Z"),/Until.*7.*2026/);
});
