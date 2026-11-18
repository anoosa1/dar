import assert from "node:assert/strict";
import {test} from "node:test";
import {callingCode,internationalDigits,localName,chatTimeKey} from "../shared/chat.ts";

test("Chat matches Dar contact numbers to WhatsApp numbers",()=>{
  assert.equal(callingCode("15550001111"),"1");assert.equal(callingCode("966551230142"),"966");assert.equal(callingCode("447700900123"),"44");assert.equal(callingCode("79161234567"),"7");
  const saudi="966551230142",us="15550001111";
  assert.equal(internationalDigits("+1 (555) 000-2222",saudi),"15550002222","international numbers keep their country");
  assert.equal(internationalDigits("00 44 7700 900123",saudi),"447700900123","00 is international");
  assert.equal(internationalDigits("055 123 4567",saudi),"966551234567","a trunk 0 takes the account's country");
  assert.equal(internationalDigits("555-000-4444",us),"15550004444","short numbers without + are national");
  assert.equal(internationalDigits("966551234567",saudi),"966551234567","long bare numbers are international");
  assert.equal(internationalDigits("12345",saudi),"","too short to be a phone number");
  assert.equal(internationalDigits("0551234567",""),"","without the account's own number a national number cannot be placed");
});

test("Chat names attachments and orders keys newest first",()=>{
  assert.equal(localName({kind:"document",mime:"application/pdf",fileName:"menu/../x.pdf",at:0}),"menu_.._x.pdf");
  assert.match(localName({kind:"voice",mime:"audio/ogg",fileName:"",at:Date.UTC(2026,10,17,10,42,5)}),/^WhatsApp Voice message 2026-11-1\d at \d\d\.\d\d\.\d\d\.ogg$/);
  assert.ok(chatTimeKey(2000)<chatTimeKey(1000));assert.equal(chatTimeKey(0).length,13);
});
