import {deflateRawSync,deflateSync,crc32} from "node:zlib";

/** A ZIP archive (stored or deflated entries) for EPUB and CBZ fixtures. */
export function zip(entries:[string,string|Buffer,boolean?][]){
  const locals:Buffer[]=[],central:Buffer[]=[];let offset=0;
  for(const [name,content,store] of entries){
    const data=Buffer.from(content),packed=store?data:deflateRawSync(data),nameBytes=Buffer.from(name),crc=crc32(data);
    const local=Buffer.alloc(30);local.writeUInt32LE(0x04034b50,0);local.writeUInt16LE(20,4);local.writeUInt16LE(0x800,6);local.writeUInt16LE(store?0:8,8);
    local.writeUInt32LE(crc,14);local.writeUInt32LE(packed.length,18);local.writeUInt32LE(data.length,22);local.writeUInt16LE(nameBytes.length,26);
    const head=Buffer.alloc(46);head.writeUInt32LE(0x02014b50,0);head.writeUInt16LE(20,4);head.writeUInt16LE(20,6);head.writeUInt16LE(0x800,8);head.writeUInt16LE(store?0:8,10);
    head.writeUInt32LE(crc,16);head.writeUInt32LE(packed.length,20);head.writeUInt32LE(data.length,24);head.writeUInt16LE(nameBytes.length,28);head.writeUInt32LE(offset,42);
    locals.push(local,nameBytes,packed);central.push(head,nameBytes);offset+=30+nameBytes.length+packed.length;
  }
  const directory=Buffer.concat(central),end=Buffer.alloc(22);end.writeUInt32LE(0x06054b50,0);end.writeUInt16LE(entries.length,8);end.writeUInt16LE(entries.length,10);end.writeUInt32LE(directory.length,12);end.writeUInt32LE(offset,16);
  return Buffer.concat([...locals,directory,end]);
}
/** A small valid PNG in one colour. */
export function png(width:number,height:number,rgb:[number,number,number]){
  const chunk=(type:string,data:Buffer)=>{const length=Buffer.alloc(4);length.writeUInt32BE(data.length);const body=Buffer.concat([Buffer.from(type),data]),crc=Buffer.alloc(4);crc.writeUInt32BE(crc32(body));return Buffer.concat([length,body,crc]);};
  const header=Buffer.alloc(13);header.writeUInt32BE(width,0);header.writeUInt32BE(height,4);header[8]=8;header[9]=2;
  const rows=Buffer.alloc((width*3+1)*height);for(let y=0;y<height;y++)for(let x=0;x<width;x++)rows.set(rgb,y*(width*3+1)+1+x*3);
  return Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]),chunk("IHDR",header),chunk("IDAT",deflateSync(rows)),chunk("IEND",Buffer.alloc(0))]);
}

const chapter=(title:string,paragraphs:number)=>`<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops" lang="en"><head><title>${title}</title><style>p{color:#000}</style><script>document.title="script ran"</script></head>
<body><h1 id="top">${title}</h1>${Array.from({length:paragraphs},(_,i)=>`<p id="p${i+1}">Paragraph ${i+1} of ${title}. The sea was calm and the wind came from the west, carrying the smell of salt and tar across the deck while the crew went about their work.</p>`).join("\n")}
<p><a href="chapter2.xhtml#p3">Jump to the third paragraph of the second chapter</a> · <a href="https://example.com/outside">An outside link</a></p></body></html>`;

/** An EPUB 3 with a cover, a navigation document, Calibre series metadata and two chapters. */
export function epub(options:{title:string;author:string;series?:[string,number];drm?:boolean}){
  const opf=`<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:identifier id="id">urn:uuid:${crypto.randomUUID()}</dc:identifier><dc:title>${options.title}</dc:title><dc:creator>${options.author}</dc:creator><dc:language>en</dc:language>
<dc:publisher>Fixture Press</dc:publisher><dc:date>1902-01-01</dc:date><dc:description>&lt;p&gt;A &lt;b&gt;test&lt;/b&gt; voyage.&lt;/p&gt;</dc:description><dc:subject>Sea stories</dc:subject>
${options.series?`<meta name="calibre:series" content="${options.series[0]}"/><meta name="calibre:series_index" content="${options.series[1]}"/>`:""}<meta name="cover" content="cover-image"/></metadata>
<manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="cover-image" href="images/cover.png" media-type="image/png" properties="cover-image"/>
<item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="chapter2.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>`;
  const nav=`<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Contents</title></head><body>
<nav epub:type="toc"><ol><li><a href="chapter1.xhtml">One: Departure</a></li><li><a href="chapter2.xhtml">Two: The Storm</a><ol><li><a href="chapter2.xhtml#p3">The mast breaks</a></li></ol></li></ol></nav></body></html>`;
  return zip([["mimetype","application/epub+zip",true],["META-INF/container.xml",`<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>`],
    ...options.drm?[["META-INF/encryption.xml",`<?xml version="1.0"?><encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/><enc:CipherData><enc:CipherReference URI="OEBPS/chapter1.xhtml"/></enc:CipherData></enc:EncryptedData></encryption>`] as [string,string]]:[],
    ["OEBPS/content.opf",opf],["OEBPS/nav.xhtml",nav],["OEBPS/images/cover.png",png(60,90,[40,64,111])],["OEBPS/chapter1.xhtml",chapter("Departure",40)],["OEBPS/chapter2.xhtml",chapter("The Storm",60)]]);
}
/** A comic book ZIP with ComicInfo.xml (manga, right to left) and pages in natural order. */
export function cbz(){
  return zip([["ComicInfo.xml",`<?xml version="1.0"?><ComicInfo><Series>Fixture Comics</Series><Number>2</Number><Writer>Ada Panel</Writer><Manga>YesAndRightToLeft</Manga><Summary>Three pages.</Summary></ComicInfo>`],
    ["pages/page10.png",png(40,60,[200,40,40])],["pages/page2.png",png(40,60,[40,200,40])],["pages/page1.png",png(40,60,[40,40,200])],["__MACOSX/pages/._page1.png","junk"]]);
}
/** A two-page PDF with a title and author, with correct cross-reference offsets. */
export function pdf(){
  const objects=["<< /Type /Catalog /Pages 2 0 R >>","<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 7 0 R >> >> >>",
    stream("BT /F1 24 Tf 40 340 Td (Page one) Tj ET"),
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>",
    stream("BT /F1 24 Tf 40 340 Td (Page two) Tj ET"),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>","<< /Title (A Fixture Paper) /Author (Grace Columns) >>"];
  let body="%PDF-1.4\n";const offsets:number[]=[];
  objects.forEach((object,index)=>{offsets.push(Buffer.byteLength(body));body+=`${index+1} 0 obj\n${object}\nendobj\n`;});
  const xref=Buffer.byteLength(body);
  body+=`xref\n0 ${objects.length+1}\n0000000000 65535 f \n${offsets.map(o=>`${String(o).padStart(10,"0")} 00000 n \n`).join("")}trailer\n<< /Size ${objects.length+1} /Root 1 0 R /Info 8 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(body,"latin1");
}
function stream(text:string){return `<< /Length ${text.length} >>\nstream\n${text}\nendstream`;}
