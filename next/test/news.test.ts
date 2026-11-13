import assert from "node:assert/strict";
import {test} from "node:test";
import http from "node:http";
import zlib from "node:zlib";
import {sanitize,textOf,discoverFeeds,extractArticle,decode} from "../runtime/news/html.ts";
import {parseFeed} from "../runtime/news/feed.ts";
import {parseOpml,exportOpml} from "../runtime/news/opml.ts";
import {safeFetch,publicAddress} from "../runtime/news/net.ts";

test("sanitizer keeps formatting and drops active content",()=>{
  const out=sanitize(`<div onclick="x()"><p style="color:red">Hi <b>there<i>!</b></p><script>alert(1)</script><style>p{}</style><iframe src="https://evil"></iframe>
    <a href="javascript:alert(1)">bad</a><a href="/post?a=1&amp;b=2" target="_blank">good</a><img src="data:image/png;base64,AA" data-src="img/a.png" onerror="x()"><img src="https://cdn.example/b.jpg" width="640" height="x">
    <form><input value="secret"></form><svg><script>bad()</script></svg><unknown-tag>kept text</unknown-tag><p>&lt;not a tag&gt; &amp; &hellip;</p></div>`,"https://site.example/blog/");
  assert.ok(!/script|style=|onclick|onerror|iframe|javascript:|<form|<input|<svg|secret|bad\(\)/.test(out),out);
  assert.match(out,/<a href="https:\/\/site\.example\/post\?a=1&amp;b=2">good<\/a>/);assert.match(out,/<a>bad<\/a>/);
  assert.match(out,/<img src="https:\/\/site\.example\/blog\/img\/a\.png">/);assert.match(out,/<img src="https:\/\/cdn\.example\/b\.jpg" width="640">/);
  assert.match(out,/<p>Hi <b>there<i>!<\/i><\/b><\/p>/);assert.match(out,/kept text/);assert.match(out,/&lt;not a tag&gt; &amp; …/);
  assert.equal(sanitize("<p>open <em>tags",""),"<p>open <em>tags</em></p>");assert.equal(decode("&#x1F600;&#0;&bogus;"),"😀�&bogus;");
  assert.equal(textOf("<p>One</p><p>Two <script>no</script>three</p>"),"One Two three");
});

test("feeds: RSS 2.0, Atom, RDF and JSON Feed with enclosures, dates and charsets",()=>{
  const rss=parseFeed(Buffer.from(`<?xml version="1.0"?><rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:media="http://search.yahoo.com/mrss/"><channel><title>Example &amp; Co</title><link>https://example.com/</link><language>ar</language>
    <item><title>First</title><link>/posts/1</link><guid isPermaLink="false">post-1</guid><pubDate>Tue, 10 Nov 2026 09:00:00 GMT</pubDate><dc:creator>Sara</dc:creator><description>Short</description><content:encoded><![CDATA[<p>Full <script>x</script>body</p>]]></content:encoded>
    <enclosure url="https://cdn.example.com/ep1.mp3" type="audio/mpeg" length="1"/><media:thumbnail url="https://cdn.example.com/t.jpg"/></item>
    <item><title>Second &nbsp; entity</title><link>https://example.com/posts/2</link><description>&lt;b&gt;Bold&lt;/b&gt; summary</description></item></channel></rss>`),"application/rss+xml","https://example.com/feed",1000);
  assert.equal(rss.kind,"RSS");assert.equal(rss.title,"Example & Co");assert.equal(rss.entries.length,2);
  const [first,second]=rss.entries;
  assert.equal(first.url,"https://example.com/posts/1");assert.equal(first.guid,"post-1");assert.equal(first.author,"Sara");assert.equal(first.pubDate,Date.parse("2026-11-10T09:00:00Z"));
  assert.equal(first.body,"<p>Full body</p>");assert.equal(first.enclosureLink,"https://cdn.example.com/ep1.mp3");assert.equal(first.enclosureMime,"audio/mpeg");assert.equal(first.mediaThumbnail,"https://cdn.example.com/t.jpg");
  assert.equal(first.rtl,true);assert.match(first.guidHash,/^[a-f0-9]{32}$/);assert.equal(second.guid,"https://example.com/posts/2");assert.equal(second.body,"<b>Bold</b> summary");assert.equal(second.pubDate,1000);
  const atom=parseFeed(Buffer.from(`<feed xmlns="http://www.w3.org/2005/Atom"><title type="text">Atom site</title><link href="https://atom.example/"/><link rel="self" href="https://atom.example/feed"/>
    <entry><id>tag:atom.example,2026:1</id><title type="html">A &lt;em&gt;title&lt;/em&gt;</title><link rel="alternate" href="https://atom.example/1"/><link rel="enclosure" href="https://atom.example/v.mp4" type="video/mp4"/><updated>2026-11-11T10:00:00Z</updated><author><name>Omar</name></author>
    <content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><p>XHTML <a href="/x">link</a></p></div></content></entry></feed>`),"application/atom+xml","https://atom.example/feed");
  assert.equal(atom.kind,"Atom");assert.equal(atom.link,"https://atom.example/");const a=atom.entries[0];
  assert.equal(a.title,"A title");assert.equal(a.author,"Omar");assert.equal(a.enclosureMime,"video/mp4");assert.match(a.body,/<p>XHTML <a href="https:\/\/atom\.example\/x">link<\/a><\/p>/);assert.equal(a.pubDate,Date.parse("2026-11-11T10:00:00Z"));
  const rdf=parseFeed(Buffer.from(`<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns="http://purl.org/rss/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/"><channel rdf:about="https://rdf.example/"><title>RDF</title><link>https://rdf.example/</link></channel><item rdf:about="https://rdf.example/a"><title>Item A</title><link>https://rdf.example/a</link><dc:date>2026-11-09T08:00:00Z</dc:date></item></rdf:RDF>`),"application/rdf+xml","https://rdf.example/rss");
  assert.equal(rdf.entries[0].url,"https://rdf.example/a");assert.equal(rdf.entries[0].pubDate,Date.parse("2026-11-09T08:00:00Z"));
  const json=parseFeed(Buffer.from(JSON.stringify({version:"https://jsonfeed.org/version/1.1",title:"JSON",home_page_url:"https://json.example/",items:[{id:7,url:"https://json.example/7",title:"Seven",content_text:"Plain\n\nText",date_published:"2026-11-08T00:00:00Z",authors:[{name:"Ali"}],attachments:[{url:"https://json.example/a.mp3",mime_type:"audio/mpeg"}]}]})),"application/feed+json","https://json.example/feed.json");
  assert.equal(json.kind,"JSON Feed");assert.equal(json.entries[0].guid,"7");assert.equal(json.entries[0].body,"<p>Plain</p><p>Text</p>");assert.equal(json.entries[0].author,"Ali");assert.equal(json.entries[0].enclosureLink,"https://json.example/a.mp3");
  const latin=Buffer.concat([Buffer.from(`<?xml version="1.0" encoding="windows-1252"?><rss><channel><title>Caf`),Buffer.from([0xe9]),Buffer.from(`</title><item><title>x</title><link>https://l.example/</link></item></channel></rss>`)]);
  assert.equal(parseFeed(latin,"text/xml","https://l.example/f").title,"Café");
  assert.throws(()=>parseFeed(Buffer.from("<html><body>hi</body></html>"),"text/html","https://x.example/"),/not an RSS/);
});

test("discovery, article extraction and OPML",()=>{
  assert.deepEqual(discoverFeeds(`<html><head><link rel="alternate" type="application/rss+xml" title="Posts" href="/feed.xml"><link rel="alternate" type="application/atom+xml" href="https://other.example/atom"><link rel="stylesheet" href="/s.css"></head><body></body></html>`,"https://site.example/blog/"),
    [{url:"https://site.example/feed.xml",title:"Posts",type:"application/rss+xml"},{url:"https://other.example/atom",title:"",type:"application/atom+xml"}]);
  const paragraph=(n:number)=>`<p>${"Paragraph text that is long enough to count as real article content. ".repeat(3)}${n}</p>`;
  const page=`<html><body><nav class="menu"><p>${"Navigation link text repeated many times. ".repeat(20)}</p></nav><div id="content">${[1,2,3,4,5].map(paragraph).join("")}<script>track()</script></div><div class="comments">${paragraph(9)}</div></body></html>`;
  const article=extractArticle(page,"https://site.example/");assert.match(article,/Paragraph text/);assert.ok(!/Navigation|track|9<\/p>/.test(article));
  assert.match(extractArticle(`<body><div>${paragraph(1)}</div><article><h1>Title</h1>${[1,2,3,4,5,6].map(paragraph).join("")}</article></body>`,"https://a.example/"),/^<h1>Title<\/h1>/);
  assert.equal(extractArticle("<p>short</p>","https://a.example/"),"");
  const feeds=parseOpml(`<?xml version="1.0"?><opml version="2.0"><body><outline text="Tech"><outline text="Sub"><outline type="rss" text="LWN" xmlUrl="https://lwn.net/headlines/rss"/></outline></outline><outline text="Loose" xmlUrl="https://loose.example/feed"/><outline text="Bad" xmlUrl="file:///etc/passwd"/></body></opml>`);
  assert.deepEqual(feeds,[{url:"https://lwn.net/headlines/rss",title:"LWN",folder:"Tech"},{url:"https://loose.example/feed",title:"Loose",folder:""}]);
  assert.throws(()=>parseOpml(`<!DOCTYPE x [<!ENTITY a "b">]><opml/>`),/document type/);
  const exported=exportOpml({folders:[{id:1,name:"Tech & Code"}],feeds:[{url:"https://lwn.net/headlines/rss",title:"LWN",link:"https://lwn.net/",folderId:1},{url:"https://loose.example/feed",title:"Loose",link:"",folderId:0}]},"admin");
  assert.deepEqual(parseOpml(exported),[{url:"https://lwn.net/headlines/rss",title:"LWN",folder:"Tech & Code"},{url:"https://loose.example/feed",title:"Loose",folder:""}]);
});

test("fetches refuse private networks unless allowed, follow checked redirects and bound size",async()=>{
  for(const address of ["127.0.0.1","10.1.2.3","192.168.1.1","169.254.169.254","::1","fd00::1","::ffff:127.0.0.1","::ffff:7f00:1","0.0.0.0","fe80::1"])assert.equal(publicAddress(address),false,address);
  for(const address of ["93.184.216.34","2606:4700::1","::ffff:93.184.216.34"])assert.equal(publicAddress(address),true,address);
  const server=http.createServer((req,res)=>{
    if(req.url==="/feed"){res.writeHead(200,{"content-type":"application/rss+xml","etag":'"v1"',"content-encoding":"gzip"});res.end(zlib.gzipSync("<rss><channel><title>T</title></channel></rss>"));return;}
    if(req.url==="/cached"){res.writeHead(req.headers["if-none-match"]==='"v1"'?304:200,{etag:'"v1"'});res.end();return;}
    if(req.url==="/redirect"){res.writeHead(302,{location:"/feed"});res.end();return;}
    if(req.url==="/metadata"){res.writeHead(302,{location:"http://169.254.169.254/latest"});res.end();return;}
    res.writeHead(200,{"content-type":"text/plain"});res.end("x".repeat(2048));
  });
  await new Promise<void>(r=>server.listen(0,"127.0.0.1",r));const base=`http://127.0.0.1:${(server.address() as import("node:net").AddressInfo).port}`;
  try{
    await assert.rejects(safeFetch(base+"/feed"),/private or local/);
    await assert.rejects(safeFetch(`http://localhost:${new URL(base).port}/feed`),/private or local/);
    const fetched=await safeFetch(base+"/redirect",{allowPrivate:true});assert.equal(fetched.status,200);assert.equal(fetched.url,base+"/feed");assert.match(fetched.body.toString(),/<title>T<\/title>/);assert.equal(fetched.etag,'"v1"');
    assert.equal((await safeFetch(base+"/cached",{allowPrivate:true,etag:'"v1"'})).status,304);
    await assert.rejects(safeFetch(base+"/big",{allowPrivate:true,maxBytes:1024}),/too large/);
    await assert.rejects(safeFetch("ftp://example.com/x"),/http and https/);await assert.rejects(safeFetch("https://user:pw@example.com/"),/without credentials/);
    // A public-looking start must not reach link-local metadata through a redirect.
    await assert.rejects(safeFetch(base+"/metadata",{allowPrivate:false}),/private or local/);
  }finally{server.close();}
});
