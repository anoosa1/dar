import assert from "node:assert/strict";
import {test} from "node:test";
import {applyPatch,filterPredicate,normalize,project,PATCH,USER,GROUP,ENTERPRISE} from "../runtime/identity/scim-model.ts";
test("SCIM validation keeps credentials write-only and rejects privilege fields and ambiguous primaries",()=>{
  const value=normalize("Users",{schemas:[USER,ENTERPRISE],USERNAME:"Alice",displayName:"Alice",password:"a private password",groups:[{value:"ignored"}],[ENTERPRISE]:{employeeNumber:"7"}});
  assert.equal(value.document.userName,"alice");assert.equal(value.password,"a private password");assert.equal(value.document.password,undefined);assert.equal(value.document.groups,undefined);
  assert.throws(()=>normalize("Users",{userName:"test",administrator:true}));assert.throws(()=>normalize("Users",{userName:"test",emails:[{value:"a@b",primary:true},{value:"c@d",primary:true}]}));
  assert.throws(()=>normalize("Groups",{schemas:[GROUP],displayName:"Nested",members:[{value:"group",type:"Group"}]}));
});
test("SCIM filters and atomic PATCH support common provisioner paths without ignoring invalid expressions",()=>{
  const user={userName:"Alice",active:true,name:{givenName:"Alice"},emails:[{value:"alice@example.com",type:"work"},{value:"home@example.com",type:"home"}]};
  for(const filter of ['userName eq "ALICE"','active eq true and emails[type eq "work" and value co "example"]','not (active eq false) and (userName sw "al" or userName ew "x")','name.givenName pr'])assert.equal(filterPredicate(filter)(user),true,filter);
  for(const filter of ['userName eq','userName bad "Alice"','active eq true ignored','(active eq true','x eq {}'])assert.throws(()=>filterPredicate(filter),filter);
  const patched=applyPatch("Users",user,{schemas:[PATCH],Operations:[{op:"replace",path:'emails[type eq "work"].value',value:"new@example.com"},{op:"remove",path:'emails[type eq "home"]'},{op:"replace",path:"name.familyName",value:"Lee"}]});
  assert.deepEqual(patched.emails,[{type:"work",value:"new@example.com"}]);assert.equal((patched.name as any).familyName,"Lee");assert.equal(user.emails.length,2);
  assert.throws(()=>applyPatch("Users",user,{schemas:[PATCH],Operations:[{op:"add",path:"name.__proto__.polluted",value:true}]}));assert.equal(({} as any).polluted,undefined);
  assert.deepEqual(project({...user,id:"id",schemas:[USER]},new URL("http://test/?attributes=userName,name.givenName")),{id:"id",schemas:[USER],userName:"Alice",name:{givenName:"Alice"}});
  assert.deepEqual(project({...user,id:"id",schemas:[USER]},new URL("http://test/?attributes=emails.value")),{id:"id",schemas:[USER],emails:[{value:"alice@example.com"},{value:"home@example.com"}]});
  const primary=normalize("Users",applyPatch("Users",{...user,emails:user.emails.map((e,i)=>({...e,primary:i===0}))},{schemas:[PATCH],Operations:[{op:"replace",path:'emails[type eq "home"].primary',value:true}]}));
  assert.equal((primary.document.emails as any[]).filter(e=>e.primary).length,1);assert.equal((primary.document.emails as any[])[1].primary,true);
});
