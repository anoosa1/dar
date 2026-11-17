import type { IncomingMessage } from "node:http";
import { SaxesParser } from "saxes";
import { AppError } from "./errors.ts";

/** A parsed WebDAV request body: namespaced elements, attributes without a namespace, text. No DTDs. */
export interface Xml { uri: string; name: string; attrs: Record<string, string>; text: string; children: Xml[] }
export function xml(text: string): Xml {
  const stack: Xml[] = []; let root: Xml | undefined, nodes = 0;
  const parser = new SaxesParser({ xmlns: true });
  parser.on("doctype", () => { throw new AppError("invalidXml"); });
  parser.on("error", () => { throw new AppError("invalidXml"); });
  parser.on("opentag", tag => {
    if (++nodes > 10000 || stack.length > 32) throw new AppError("requestTooLarge", 413);
    const node = { uri: tag.uri, name: tag.local, attrs: Object.fromEntries(Object.values(tag.attributes).filter(a => !a.uri).map(a => [a.local, a.value])), text: "", children: [] } as Xml;
    if (stack.length) stack.at(-1)!.children.push(node); else { if (root) throw new AppError("invalidXml"); root = node; }
    stack.push(node);
  });
  parser.on("text", text => { if (stack.length) stack.at(-1)!.text += text; });
  parser.on("cdata", text => { if (stack.length) stack.at(-1)!.text += text; });
  parser.on("closetag", () => { stack.pop(); });
  try { parser.write(text).close(); } catch (error) { if (error instanceof AppError) throw error; throw new AppError("invalidXml"); }
  if (!root) throw new AppError("invalidXml"); return root;
}
export const children = (node: Xml | undefined, uri: string, name: string) => node?.children.filter(n => n.uri === uri && n.name === name) ?? [];
export async function readBody(req: IncomingMessage, limit = 262144) {
  const chunks: Buffer[] = []; let size = 0;
  for await (const chunk of req.iterator({ destroyOnReturn: false })) { size += chunk.length; if (size > limit) { req.resume(); throw new AppError("requestTooLarge", 413); } chunks.push(chunk); }
  return Buffer.concat(chunks).toString("utf8");
}
