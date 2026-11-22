// Compares the Rust module's schema with the TypeScript module's: tables (columns, types,
// primary keys, indexes, unique constraints, sequences, access), views, reducers and
// procedures with their parameters. The Rust module must match exactly so a TypeScript
// database can be upgraded to it and TypeScript bindings read it.
// Usage (from Dar's root dev shell, modules built): node rust/tools/schema-diff.mjs [--json]
import {spawn, spawnSync} from "node:child_process";
import {mkdtemp, writeFile} from "node:fs/promises";
import {tmpdir} from "node:os";
import net from "node:net";
import path from "node:path";

const dar = path.resolve(import.meta.dirname, "../..");
const typescript = path.join(dar, "next/module/dist/bundle.js"), rust = path.join(dar, "rust/module/target/wasm32-unknown-unknown/release/dar_module.wasm");
const root = await mkdtemp(path.join(tmpdir(), "dar-schema-"));
const port = await new Promise(resolve => { const s = net.createServer(); s.listen(0, "127.0.0.1", () => { const {port} = s.address(); s.close(() => resolve(port)); }); });
const url = `http://127.0.0.1:${port}`, cli = process.env.DAR_SPACETIME_BIN ?? "spacetime", cliRoot = path.join(root, "cli");
const server = spawn(cli, ["--root-dir", cliRoot, "start", "--data-dir", path.join(root, "data"), "--listen-addr", `127.0.0.1:${port}`, "--non-interactive"], {stdio: "ignore"});
try {
  for (let i = 0; ; i++) { try { if ((await fetch(url + "/v1/ping")).ok) break; } catch {} if (i > 300) throw Error("SpacetimeDB did not start"); await new Promise(r => setTimeout(r, 100)); }
  const schema = async (name, args) => {
    const r = spawnSync(cli, ["--root-dir", cliRoot, "publish", name, ...args, "--server", url, "--yes", "--no-config"], {env: {...process.env, DAR_ISSUER: "http://127.0.0.1:1/auth/issuer"}, encoding: "utf8"});
    if (r.status !== 0) throw Error(`${name} did not publish:\n${r.stdout}${r.stderr}`);
    const response = await fetch(`${url}/v1/database/${name}/schema?version=9`);
    if (!response.ok) throw Error(`${name} schema: ${response.status} ${await response.text()}`);
    return response.json();
  };
  const [ts, rs] = [await schema("schema-ts", ["--js-path", typescript]), await schema("schema-rs", ["--bin-path", rust])];
  await writeFile(path.join(root, "typescript.json"), JSON.stringify(ts, null, 1)); await writeFile(path.join(root, "rust.json"), JSON.stringify(rs, null, 1));
  // Resolve type references to readable shapes.
  const typeName = (schema, t) => {
    if (t === undefined || t === null) return "?";
    const [kind, value] = Object.entries(t)[0];
    if (kind === "Ref") { const def = schema.typespace.types[value]; const named = schema.types?.find(x => x.ty === value); return named ? named.name.name : typeName(schema, def); }
    if (kind === "Array") return `Array<${typeName(schema, value)}>`;
    if (kind === "Product") return `{${value.elements.map(e => `${e.name?.some ?? e.name ?? "_"}:${typeName(schema, e.algebraic_type)}`).join(",")}}`;
    if (kind === "Sum") return value.variants.map(v => `${v.name?.some ?? v.name}(${typeName(schema, v.algebraic_type)})`).join("|");
    return kind;
  };
  const columns = (schema, table) => { const product = schema.typespace.types[table.product_type_ref].Product; return product.elements.map(e => `${e.name?.some ?? e.name}:${typeName(schema, e.algebraic_type)}`); };
  const describeTables = schema => Object.fromEntries(schema.tables.map(table => [table.name, {
    columns: columns(schema, table),
    primaryKey: JSON.stringify(table.primary_key),
    indexes: table.indexes.map(i => JSON.stringify([i.algorithm, i.accessor_name ?? null])).sort(),
    constraints: table.constraints.map(c => JSON.stringify(c.data)).sort(),
    sequences: table.sequences.map(s => s.column).sort(),
    access: JSON.stringify(table.table_access), type: JSON.stringify(table.table_type),
  }]));
  const params = (schema, p) => (p.elements ?? []).map(e => `${e.name?.some ?? e.name}:${typeName(schema, e.algebraic_type)}`).join(", ");
  const describeFunctions = (schema, list, kind) => Object.fromEntries((list ?? []).map(f => [f.name, `${kind}(${params(schema, f.params)})${f.return_type ? ` -> ${typeName(schema, f.return_type)}` : ""}${f.lifecycle?.some ? ` [${JSON.stringify(f.lifecycle.some)}]` : ""}`]));
  const misc = (schema, kind) => (schema.misc_exports ?? []).filter(e => e[kind]).map(e => e[kind]);
  const functions = schema => ({...describeFunctions(schema, schema.reducers, "reducer"), ...describeFunctions(schema, misc(schema, "Procedure"), "procedure"), ...describeFunctions(schema, misc(schema, "View"), "view")});
  const defaults = schema => Object.fromEntries(misc(schema, "ColumnDefaultValue").map(d => [`${d.table}.${d.col_id}`, JSON.stringify(d.value)]));
  const report = {missingTables: [], extraTables: [], tableDifferences: {}, missingFunctions: [], extraFunctions: [], functionDifferences: {}};
  const [tt, rt] = [describeTables(ts), describeTables(rs)];
  for (const name of Object.keys(tt)) {
    if (!rt[name]) { report.missingTables.push(name); continue; }
    const diff = Object.fromEntries(Object.keys(tt[name]).filter(k => JSON.stringify(tt[name][k]) !== JSON.stringify(rt[name][k])).map(k => [k, {typescript: tt[name][k], rust: rt[name][k]}]));
    if (Object.keys(diff).length) report.tableDifferences[name] = diff;
  }
  report.extraTables = Object.keys(rt).filter(name => !tt[name]);
  const [tf, rf] = [functions(ts), functions(rs)];
  for (const name of Object.keys(tf)) { if (!rf[name]) report.missingFunctions.push(name); else if (tf[name] !== rf[name]) report.functionDifferences[name] = {typescript: tf[name], rust: rf[name]}; }
  report.extraFunctions = Object.keys(rf).filter(name => !tf[name]);
  const [td, rd] = [defaults(ts), defaults(rs)];
  report.defaultDifferences = Object.fromEntries([...new Set([...Object.keys(td), ...Object.keys(rd)])].filter(k => td[k] !== rd[k]).map(k => [k, {typescript: td[k], rust: rd[k]}]));
  if (process.argv.includes("--json")) console.log(JSON.stringify(report, null, 1));
  else {
    console.log(`Tables: ${Object.keys(tt).length} TypeScript, ${Object.keys(rt).length} Rust; ${report.missingTables.length} missing, ${Object.keys(report.tableDifferences).length} different, ${report.extraTables.length} extra.`);
    console.log(`Column defaults: ${Object.keys(report.defaultDifferences).length} different.`);
    console.log(`Functions: ${Object.keys(tf).length} TypeScript, ${Object.keys(rf).length} Rust; ${report.missingFunctions.length} missing, ${Object.keys(report.functionDifferences).length} different, ${report.extraFunctions.length} extra.`);
    if (report.missingTables.length) console.log("\nMissing tables:", report.missingTables.join(", "));
    if (report.extraTables.length) console.log("\nExtra tables:", report.extraTables.join(", "));
    for (const [name, diff] of Object.entries(report.tableDifferences)) { console.log(`\nTable ${name}:`); for (const [k, v] of Object.entries(diff)) console.log(`  ${k}:\n    ts: ${JSON.stringify(v.typescript)}\n    rs: ${JSON.stringify(v.rust)}`); }
    if (report.missingFunctions.length) console.log("\nMissing functions:", report.missingFunctions.join(", "));
    if (report.extraFunctions.length) console.log("\nExtra functions:", report.extraFunctions.join(", "));
    for (const [name, diff] of Object.entries(report.functionDifferences)) console.log(`\n${name}:\n  ts: ${diff.typescript}\n  rs: ${diff.rust}`);
    for (const [name, diff] of Object.entries(report.defaultDifferences)) console.log(`\nColumn default ${name}: ts ${diff.typescript} rs ${diff.rust}`);
    console.log(`\nRaw schemas: ${root}`);
  }
} finally { server.kill("SIGTERM"); }
