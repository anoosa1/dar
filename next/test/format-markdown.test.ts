import {test} from "node:test";
import assert from "node:assert/strict";
import {formatMarkdown} from "../client/pim/format-markdown.ts";

test("block formatting starts at line boundaries and preserves unselected CRLF source",()=>{
  const source="Before\r\nfirst line\r\nsecond line\r\nAfter";
  const start=source.indexOf("line"),end=source.indexOf("\r\nAfter");
  assert.equal(formatMarkdown(source,start,end,"- ","").text,"Before\r\n- first line\r\n- second line\r\nAfter");
  assert.equal(formatMarkdown("a sentence",3,3,"# ","").text,"# a sentence");
  assert.equal(formatMarkdown("one\ntwo\n",0,4,"> ","").text,"> one\ntwo\n");
  assert.equal(formatMarkdown("\ntext",0,0,"- ","").text,"- \ntext");
});
test("inline formatting wraps only the selection and preserves caret selection",()=>{
  assert.deepEqual(formatMarkdown("before text after",7,11,"**","**"),{text:"before **text** after",start:9,end:13});
});
