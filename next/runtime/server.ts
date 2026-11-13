import http, { type IncomingMessage, type ServerResponse } from "node:http";
import { readFile } from "node:fs/promises";
import { timingSafeEqual, randomUUID } from "node:crypto";
import path from "node:path";
import { isIP } from "node:net";
import { Context } from "cordis";
import { database } from "./database.ts";
import { openRoot } from "./root.ts";
import { createIssuer } from "./auth/issuer.ts";
import { authService } from "./auth/service.ts";
import { authorizeCookieWrite } from "./auth/session.ts";
import { AppError } from "./errors.ts";
import { blobStore, type BlobFile } from "./files/blobs.ts";
import { lockRoot } from "./lock.ts";
import { handleDav } from "./files/dav.ts";
import { createOpaqueToken } from "./auth/session.ts";
import { reconcileStorage } from "./files/maintenance.ts";
import { handlePimHttp } from "./pim/http.ts";
import { pimService } from "./pim/service.ts";
import { deliverReminders } from "./pim/reminders.ts";
import { handlePimDav } from "./pim/dav.ts";
import { identityProvider } from "./identity/provider.ts";
import { handleIdentityHttp } from "./identity/http.ts";
import { handleScim } from "./identity/scim.ts";
import {handleWorkspaceHttp} from "./pim/workspace-http.ts";
import {passwordApps,passwordGrants} from "../shared/app-scopes.ts";
import {loadApps} from "./apps/registry.ts";
import {handleAppHttp} from "./apps/http.ts";
import {activateApps} from "./apps/server.ts";
import type {AppSettings,ManagedApp} from "../shared/app-settings.ts";
import { configureOidc } from "./identity/config.ts";
import {handleAgentHttp} from "./agent/http.ts";
import {createAgentRuntime,type AgentRuntime} from "./agent/runtime.ts";
import {createNewsRuntime,type NewsRuntime} from "./news/runtime.ts";
import {handleNewsHttp} from "./news/http.ts";
import {createPhotosRuntime,type PhotosRuntime} from "./photos/runtime.ts";
import {handlePhotosHttp} from "./photos/http.ts";
import {handleNextcloudNews,nextcloudBase} from "./news/nextcloud.ts";

export interface ServerConfig {
  host: string; port: number; origin: string; root: string;
  databaseUrl: string; databaseName: string; serviceTokenFile: string; bootstrapTokenFile: string;
  databasePublicUrl?: string;
  development?: boolean;
  appsFile?:string;
  oidcFile?: string;
  codexFile?:string;
  claudeFile?:string;
  /** Operator choice: let News fetch feeds on private/loopback networks (off by default). */
  newsAllowPrivateNetworks?:boolean;
}
function json(res: ServerResponse, value: unknown, status = 200) {
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" });
  res.end(JSON.stringify(value));
}
async function body(req: IncomingMessage, limit = 16_384): Promise<Record<string, unknown>> {
  if (!req.headers["content-type"]?.startsWith("application/json")) throw new AppError("invalidContentType", 415);
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of req.iterator({ destroyOnReturn: false })) {
    size += chunk.length;
    if (size > limit) { req.resume(); throw new AppError("requestTooLarge", 413); }
    chunks.push(chunk);
  }
  try {
    const value: unknown = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error();
    return value as Record<string, unknown>;
  } catch { throw new AppError("invalidJson"); }
}
function string(value: unknown, max = 1024): string {
  if (typeof value !== "string" || Buffer.byteLength(value) > max) throw new AppError("invalidInput");
  return value;
}
function integer(value: unknown, max: number): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0 || value > max) throw new AppError("invalidInput");
  return value;
}

export async function startServer(config: ServerConfig) {
  const baseUrl = new URL(config.origin);
  if (baseUrl.pathname !== "/" || baseUrl.search || baseUrl.hash || baseUrl.username || baseUrl.password) throw new Error("DAR_ORIGIN must be an origin without a path or credentials.");
  config = { ...config, origin: baseUrl.origin };
  if (!config.development && new URL(config.origin).protocol !== "https:") throw new Error("Production requires an HTTPS origin.");
  if (config.development && !["127.0.0.1", "localhost", "[::1]"].includes(new URL(config.origin).hostname)) throw new Error("Development HTTP is loopback-only.");
  if (config.development && !["127.0.0.1", "::1", "localhost"].includes(config.host)) throw new Error("Development HTTP must bind to loopback.");
  const root = await openRoot(config.root);
  const releaseRoot = await lockRoot(root);
  let ownedContext: Context | undefined;
  let appRuntime:Awaited<ReturnType<typeof activateApps>>|undefined;
  let agentRuntime:AgentRuntime|undefined;
  let newsRuntime:NewsRuntime|undefined;
  let photosRuntime:PhotosRuntime|undefined;
  try {
  const db = await database(config.databaseUrl, config.databaseName, config.serviceTokenFile);
  await db.call("service_startup", [`${config.origin}/auth/issuer`]);
  const appRegistry=await loadApps(config.appsFile??path.join(root,"apps.json")).catch(error=>{
    if(error.code==="ENOENT")return {apps:new Map(),inventory:[]};
    console.warn("Installed app registry is unavailable; starting without external apps.");
    return {apps:new Map(),inventory:[{id:"registry",name:"App registry",version:"",status:"Invalid registry. Check host configuration.",permissions:[]}]};
  });
  const readAppSettings=()=>db.call<AppSettings>("service_app_settings",["","",false,0]);
  const initialAppSettings=await readAppSettings();
  const hostEnabledApps=new Set(appRegistry.apps.keys());
  appRuntime=await activateApps(appRegistry,initialAppSettings.disabled);
  await db.call("service_installed_apps",[JSON.stringify([...appRegistry.apps.values()].map(a=>a.descriptor))]);
  // Keep revisioned writes and hook disposal/activation in the same order.
  let appUpdate:Promise<unknown>=Promise.resolve();
  const managedApps=(settings:AppSettings):ManagedApp[]=>[
    ...["files","photos","contacts","calendar","notes","tasks","news","agent"].map(id=>({id,name:id[0].toUpperCase()+id.slice(1),version:"",permissions:[],status:"",enabled:!settings.disabled.includes(id),canToggle:["notes","tasks","news","photos","agent"].includes(id)})),
    ...appRegistry.inventory.map(item=>({...item,enabled:appRegistry.apps.has(item.id)&&!settings.disabled.includes(item.id),canToggle:hostEnabledApps.has(item.id)})),
  ];
  const databasePublicUrl = `${(config.databasePublicUrl ?? (config.development ? config.databaseUrl : `${config.origin}/spacetime`)).replace(/\/$/, "")}/`;
  const issuer = await createIssuer(path.join(root, "secrets"), `${config.origin}/auth/issuer`);
  const auth = await authService(db, issuer);
  const identity = await identityProvider(db,auth,path.join(root,"secrets"),config.origin,!!config.development);
  await configureOidc(config.oidcFile ?? path.join(root, "oidc.json"), identity.store, !!config.development, !config.oidcFile);
  const bootstrap = (await readFile(config.bootstrapTokenFile, "utf8")).trim();
  if (bootstrap.length < 32) throw new Error("The operator bootstrap token must have at least 32 characters.");
  const ctx = new Context();
  ownedContext = ctx;
  let disposeResources: () => Promise<void> = releaseRoot;
  // Independent Cordis effects can dispose concurrently. One ordered disposer
  // keeps the storage lock held until HTTP handlers and maintenance have drained.
  ctx.effect(() => () => disposeResources(), "runtime-resources");
  agentRuntime=await createAgentRuntime(db,root,config.codexFile??path.join(root,"codex.json"),config.claudeFile??path.join(root,"claude.json"));
  newsRuntime=createNewsRuntime(db,{allowPrivate:!!config.newsAllowPrivateNetworks});
  const blobs = blobStore(root);
  photosRuntime=await createPhotosRuntime(db,root);
  await blobs.recover();
  const attempts = new Map<string, { count: number; until: number }>();
  const cookieName = config.development ? "dar_session" : "__Host-dar_session";
  const cookie = (value: string, age: number) => `${cookieName}=${value}; Path=/; HttpOnly; SameSite=Strict; Max-Age=${age}${config.development ? "" : "; Secure"}`;
  // Only the canonical local TLS proxy may supply this overwritten client header.
  const peer = (req: IncomingMessage) => {
    const remote = req.socket.remoteAddress ?? "unknown", forwarded = req.headers["x-dar-client-ip"];
    return !config.development && ["127.0.0.1", "::1", "::ffff:127.0.0.1"].includes(remote)
      && req.headers.host === baseUrl.host && req.headers["x-forwarded-proto"] === "https"
      && typeof forwarded === "string" && isIP(forwarded) ? forwarded : remote;
  };
  const handle = async (req: IncomingMessage, res: ServerResponse) => {
    res.setHeader("x-content-type-options", "nosniff");
    res.setHeader("referrer-policy", "no-referrer");
    if (!config.development) res.setHeader("strict-transport-security", "max-age=31536000");
    const databaseOrigin = new URL(databasePublicUrl).origin;
    // The pinned SDK generates binary codecs with Function(). Keep inline
    // scripts forbidden and document this SDK-specific CSP limitation.
    res.setHeader("content-security-policy", `default-src 'self'; script-src 'self' 'unsafe-eval'; style-src 'self'; img-src 'self' blob:; media-src 'self' https:; connect-src 'self' ${databaseOrigin} ${databaseOrigin.replace(/^http/, "ws")}; frame-ancestors 'none'; base-uri 'none'; form-action 'self'`);
    try {
      const url = new URL(req.url ?? "/", config.origin);
      const method = req.method ?? "GET";
      if(url.pathname.startsWith("/appAssets/")){
        const parts=url.pathname.split("/"),app=appRegistry.apps.get(parts[2]);
        const name=parts.slice(4).join("/"),bytes=app?.descriptor.digest===parts[3]?app.assets.get(name):undefined;
        if(!bytes||!app||name===app.manifest.server||!["GET","HEAD"].includes(method))throw new AppError("notFound",404);
        const contentType=name.endsWith(".js")?"text/javascript":name.endsWith(".css")?"text/css":name.endsWith(".png")?"image/png":name.endsWith(".webp")?"image/webp":name.endsWith(".svg")?"image/svg+xml":"application/json";
        res.writeHead(200,{"content-type":contentType,"cache-control":"public, max-age=31536000, immutable"});res.end(method==="HEAD"?undefined:bytes);return;
      }
      if(url.pathname==="/oidc-form.js"&&method==="GET"){res.writeHead(200,{"content-type":"text/javascript","cache-control":"public, max-age=3600"});res.end('document.getElementById("oidc-response")?.requestSubmit();');return;}
      if(url.pathname==="/oidc"||url.pathname.startsWith("/oidc/")){
        const key=`oidc:${peer(req)}`,previous=attempts.get(key),attempt=previous&&previous.until>Date.now()?previous:{count:0,until:Date.now()+60000};
        if(++attempt.count>400||(!previous&&attempts.size>=10000))throw new AppError("rateLimited",429);attempts.set(key,attempt);
        return await identity.handle(req,res);
      }
      if(url.pathname==="/scim/v2"||url.pathname.startsWith("/scim/v2/"))return await handleScim(req,res,url,db,auth,config.origin);
      if (["/dav", "/.well-known/carddav", "/.well-known/caldav"].includes(url.pathname)) { res.writeHead(308, { location: "/dav/" }); res.end(); return; }
      if (url.pathname === nextcloudBase || url.pathname.startsWith(nextcloudBase + "/")) {
        if ((await readAppSettings()).disabled.includes("news")) throw new AppError("notFound", 404);
        return await handleNextcloudNews(req, res, url, db, newsRuntime!, config.origin);
      }
      if (url.pathname.startsWith("/dav/") && !url.pathname.startsWith("/dav/files/")) return await handlePimDav(req, res, db, config.origin);
      if (url.pathname.startsWith("/dav/")) return await handleDav(req, res, db, blobs, config.origin);
      if (url.pathname === "/auth/issuer/.well-known/openid-configuration") return json(res, { issuer: issuer.issuer, jwks_uri: `${issuer.issuer}/jwks`, id_token_signing_alg_values_supported: ["ES256"], subject_types_supported: ["public"] });
      if (url.pathname === "/auth/issuer/jwks") return json(res, issuer.jwks);
      if (url.pathname === "/api/v1/auth/status" && method === "GET") return json(res, await auth.status());
      if (url.pathname === "/api/v1/appearance" && method === "GET" || url.pathname === "/api/v1/appearance/logo" && method === "GET") {
        const row=await db.call<{revision:number;value:string}>("service_appearance",["",0,""]), value=JSON.parse(row.value);
        if(url.pathname.endsWith("/logo")){
          if(!value.logo)throw new AppError("notFound",404);
          res.writeHead(200,{"content-type":value.mediaType,"cache-control":"no-store"});res.end(Buffer.from(value.logo,"base64"));return;
        }
        return json(res,{revision:row.revision,logoUrl:value.logo?`/api/v1/appearance/logo?revision=${row.revision}`:null});
      }
      if (url.pathname.startsWith("/public/files/") && ["GET", "HEAD"].includes(method)) {
        const token = url.pathname.slice("/public/files/".length);
        if (!/^[A-Za-z0-9_-]{43}$/.test(token)) throw new AppError("notFound", 404);
        const { digestToken } = await import("./auth/session.ts");
        const entry = await db.call<BlobFile>("service_public_download", [digestToken(token)]);
        return await blobs.download(req, res, entry);
      }
      const secret = req.headers.cookie?.split(";").map(part => part.trim()).find(part => part.startsWith(`${cookieName}=`))?.slice(cookieName.length + 1);
      const origin = typeof req.headers.origin === "string" ? req.headers.origin : undefined;
      if (["POST", "PUT", "PATCH", "DELETE"].includes(method) && origin !== config.origin) throw new AppError("invalidOrigin", 403);
      if (["/api/v1/auth/login", "/api/v1/auth/setup"].includes(url.pathname) && method === "POST") {
        const key = peer(req);
        const previous = attempts.get(key);
        const attempt = previous && previous.until > Date.now() ? previous : { count: 0, until: Date.now() + 60_000 };
        if (++attempt.count > 10 || (!previous && attempts.size >= 10_000)) throw new AppError("rateLimited", 429);
        attempts.set(key, attempt);
        const p = await body(req);
        const username = string(p.username, 64), password = string(p.password);
        if (url.pathname.endsWith("/setup")) {
          const supplied = Buffer.from(string(p.bootstrapToken, 512));
          const expected = Buffer.from(bootstrap);
          if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected)) throw new AppError("forbidden", 403);
          await auth.createAccount(username, string(p.displayName, 100), password, true);
        }
        const result = await auth.login(username, password);
        attempt.count = Math.max(0, attempt.count - 1);
        res.setHeader("set-cookie", cookie(result.secret, 7 * 86400));
        return json(res, { user: result.user, csrfToken: result.csrf });
      }
      if (url.pathname.startsWith("/api/")) {
        const authenticated = await auth.authenticate(secret);
        if (!authenticated) throw new AppError("unauthenticated", 401);
        if (["POST", "PUT", "PATCH", "DELETE"].includes(method) && !authorizeCookieWrite(config.origin, origin, typeof req.headers["x-csrf-token"] === "string" ? req.headers["x-csrf-token"] : undefined, authenticated.session.csrfDigest)) throw new AppError("csrfRejected", 403);
        if(url.pathname==="/api/v1/search"&&method==="GET")return json(res,await db.call("service_global_search",[authenticated.session.id,string(url.searchParams.get("q")??"",800)]));
        if(url.pathname==="/api/v1/appSettings"&&method==="GET")return json(res,await readAppSettings());
        if(url.pathname==="/api/v1/admin/installedApps"){
          if(!authenticated.user.administrator)throw new AppError("forbidden",403);
          if(method==="GET"){const settings=await readAppSettings();return json(res,{...settings,items:managedApps(settings)});}
          if(method!=="PUT")throw new AppError("methodNotAllowed",405);
          const p=await body(req),id=string(p.id,100),revision=integer(p.revision,0xffffffff);
          if(typeof p.enabled!=="boolean")throw new AppError("invalidInput");
          if(req.headers["if-match"]!==`"apps:${revision}"`)throw new AppError("preconditionRequired",428);
          const update=appUpdate.catch(()=>{}).then(async()=>{
            if(!managedApps(await readAppSettings()).some(app=>app.id===id&&app.canToggle))throw new AppError("forbidden",403);
            const settings=await db.call<AppSettings>("service_app_settings",[authenticated.session.id,id,p.enabled,revision]);
            if(id==="agent"&&!p.enabled)agentRuntime!.cancelAll();
            await appRuntime!.setEnabled(id,p.enabled as boolean);
            await db.call("service_installed_apps",[JSON.stringify([...appRegistry.apps.values()].map(a=>a.descriptor))]);
            return {...settings,items:managedApps(settings)};
          });appUpdate=update;return json(res,await update);
        }
        if(url.pathname==="/api/v1/apps"||url.pathname.startsWith("/api/v1/apps/"))return await handleAppHttp(req,res,url,appRegistry,db,authenticated.session.id,authenticated.user,appRuntime!.handlers,()=>body(req,80*1024));
        if(url.pathname==="/api/v1/files/picker"&&method==="GET")return json(res,await db.call("service_file_picker",[authenticated.session.id,url.searchParams.get("parentId")??"",url.searchParams.get("cursor")??"",url.searchParams.get("kind")??"folder"]));
        if(url.pathname==="/api/v1/agent/provider/connect"&&method==="POST"){
          if(!authenticated.user.administrator)throw new AppError("forbidden",403);
          await db.call("service_agent",[authenticated.session.id,"projects","{}"]);
          const input=Number(req.headers["content-length"]??0)>0?await body(req):{};if(Object.keys(input).some(key=>key!=="provider")||input.provider!==undefined&&input.provider!=="codex"&&input.provider!=="claude")throw new AppError("invalidInput");
          return json(res,await agentRuntime!.connect(authenticated.user.id,input.provider==="claude"?"claude":"codex"));
        }
        if(/^\/api\/v1\/agent(\/|$)/.test(url.pathname))return await handleAgentHttp(req,res,url,db,authenticated.session.id,authenticated.user.id,agentRuntime!,()=>body(req,128*1024));
        const optionalApp=/^\/api\/v1\/(notes|tasks|news|photos)(\/|$)/.exec(url.pathname);
        if(optionalApp&&(await readAppSettings()).disabled.includes(optionalApp[1]))throw new AppError("notFound",404);
        if(/^\/api\/v1\/news(\/|$)/.test(url.pathname))return await handleNewsHttp(req,res,url,db,authenticated.session.id,authenticated.user.username,newsRuntime!,()=>body(req,3*1024*1024));
        if(/^\/api\/v1\/photos(\/|$)/.test(url.pathname))return await handlePhotosHttp(req,res,url,db,authenticated.session.id,photosRuntime!,entry=>blobs.download(req,res,entry,true),()=>body(req,128*1024));
        if(/^\/api\/v1\/(notes|tasks)(\/|$)/.test(url.pathname))return await handleWorkspaceHttp(req,res,url,db,authenticated.session.id,()=>body(req,3*1024*1024));
        if(url.pathname==="/api/v1/account/profile"){
          const profile=await db.call<{id:string;revision:number}>("service_profile",[authenticated.session.id,"read","{}"]),etag=`"${profile.id}:${profile.revision}"`;
          if(method==="GET"){res.setHeader("etag",etag);return json(res,profile);}
          if(method==="PUT"){if(!req.headers["if-match"])throw new AppError("preconditionRequired",428);if(req.headers["if-match"]!==etag)throw new AppError("revisionConflict",412);const p=await body(req);if(Object.keys(p).some(key=>!["displayName","givenName","familyName","email"].includes(key)))throw new AppError("invalidInput",400,"Only personal profile fields can be changed here.");const result=await db.call<{id:string;revision:number}>("service_profile",[authenticated.session.id,"save",JSON.stringify({revision:profile.revision,displayName:p.displayName,givenName:p.givenName??"",familyName:p.familyName??"",email:p.email??""})]);res.setHeader("etag",`"${result.id}:${result.revision}"`);return json(res,result);}
          throw new AppError("methodNotAllowed",405);
        }
        if(url.pathname.startsWith("/api/v1/identity/")){
          let attempt:{count:number;until:number}|undefined;
          if(method==="POST"&&(url.pathname.endsWith("/confirmation")||url.pathname.includes("/interactions/"))){
            const key=`identity:${peer(req)}`,previous=attempts.get(key);attempt=previous&&previous.until>Date.now()?previous:{count:0,until:Date.now()+60000};
            if(++attempt.count>20||(!previous&&attempts.size>=10000))throw new AppError("rateLimited",429);attempts.set(key,attempt);
          }
          await handleIdentityHttp(req,res,url,db,auth,authenticated,identity,config.origin,!!config.development,()=>body(req,256*1024));
          if(attempt)attempt.count=Math.max(0,attempt.count-1);return;
        }
        if (/^\/api\/v1\/(contacts|calendar|pim)(\/|$)/.test(url.pathname)) return await handlePimHttp(req, res, url, pimService(db, authenticated.session.id), () => body(req, 3 * 1024 * 1024));
        if (url.pathname === "/api/v1/auth/session" && method === "GET") return json(res, { user: authenticated.user, csrfToken: issuer.csrf(secret!) });
        if(url.pathname==="/api/v1/appearance" && method==="PUT"){
          if(!authenticated.user.administrator)throw new AppError("forbidden",403);
          const p=await body(req,360000),logo=string(p.logo,350000),mediaType=string(p.mediaType,20);
          const bytes=Buffer.from(logo,"base64");
          if(logo && (bytes.length>256*1024 || bytes.toString("base64")!==logo ||
            !(mediaType==="image/png" && bytes.subarray(0,8).equals(Buffer.from([137,80,78,71,13,10,26,10])) ||
              mediaType==="image/webp" && bytes.subarray(0,4).toString()==="RIFF" && bytes.subarray(8,12).toString()==="WEBP")))
            throw new AppError("invalidInput",400,"Choose a PNG or WebP image up to 256 KiB.");
          const revision=integer(p.revision,0xffffffff);
          if(req.headers["if-match"]!==`"appearance:${revision}"`)throw new AppError("preconditionRequired",428);
          const row=await db.call<{revision:number}>("service_appearance",[authenticated.session.id,revision,JSON.stringify({logo,mediaType:logo?mediaType:""})]);
          return json(res,{revision:row.revision,logoUrl:logo?`/api/v1/appearance/logo?revision=${row.revision}`:null});
        }
        if (url.pathname === "/api/v1/auth/token" && method === "POST") return json(res, { token: await issuer.token(authenticated.session.id, authenticated.session.expiresAt, url.searchParams.get("tab") ?? undefined), databaseUrl: databasePublicUrl, databaseName: config.databaseName });
        if (url.pathname === "/api/v1/auth/logout" && method === "POST") {
          await auth.logout(authenticated);agentRuntime!.cancelSession(authenticated.session.id); res.setHeader("set-cookie", cookie("", 0)); return json(res, { ok: true });
        }
        if (url.pathname === "/api/v1/auth/password" && method === "POST") {
          const p = await body(req); await auth.changePassword(authenticated, string(p.currentPassword), string(p.password));
          res.setHeader("set-cookie", cookie("", 0)); return json(res, { ok: true });
        }
        if (url.pathname === "/api/v1/auth/sessions" && method === "GET") return json(res, { items: await db.call("service_sessions", [authenticated.session.id]) });
        const sessionRoute = /^\/api\/v1\/auth\/sessions\/([^/]+)$/.exec(url.pathname);
        if (sessionRoute && method === "DELETE") { await db.call("service_revoke_owned_session", [authenticated.session.id, sessionRoute[1]]); return json(res, { ok: true }); }
        if (url.pathname === "/api/v1/auth/app-passwords" && method === "GET") return json(res, { items: await db.call("service_app_passwords", [authenticated.session.id]) });
        if (url.pathname === "/api/v1/auth/app-passwords" && method === "POST") {
          const p = await body(req), token = createOpaqueToken(), id = randomUUID();
          const scope = string(p.scope, 120), apps=passwordApps.filter(app=>passwordGrants(scope).some(g=>g.startsWith(app+":"))), app = apps[0] === "contacts" ? "addressbooks" : apps[0] === "calendar" ? "calendars" : "files";
          await db.call("service_create_app_password", [authenticated.session.id, id, token.digest, string(p.label, 100), scope]);
          return json(res, { id, secret: token.secret, filesDavUrl:apps.includes("files")?`${config.origin}/dav/files/${encodeURIComponent(authenticated.user.username)}/`:undefined, newsUrl:apps.includes("news")?`${config.origin}${nextcloudBase}`:undefined, davUrl: apps.length>1?`${config.origin}/dav/`:`${config.origin}/dav/${app}/${encodeURIComponent(authenticated.user.username)}/` }, 201);
        }
        const passwordRoute = /^\/api\/v1\/auth\/app-passwords\/([^/]+)$/.exec(url.pathname);
        if (passwordRoute && method === "DELETE") { await db.call("service_revoke_app_password", [authenticated.session.id, passwordRoute[1]]); return json(res, { ok: true }); }
        if (url.pathname === "/api/v1/auth/users" && method === "POST") {
          if (!authenticated.user.administrator) throw new AppError("forbidden", 403);
          const p = await body(req); const id = await auth.createAccount(string(p.username, 64), string(p.displayName, 100), string(p.password), false);
          return json(res, { id }, 201);
        }
        if (url.pathname === "/api/v1/files/uploads" && method === "POST") {
          const p = await body(req), id = randomUUID();
          const upload = await db.call("service_begin_upload", [authenticated.session.id, id,
            p.fileId ? string(p.fileId, 100) : randomUUID(), string(p.parentId ?? "", 100), string(p.name, 1024), integer(p.size, 1024 ** 3),
            string(p.mediaType ?? "application/octet-stream", 150).toLowerCase(), integer(p.expectedRevision ?? 0, 0xffffffff)]);
          return json(res, upload, 201);
        }
        const uploadRoute = /^\/api\/v1\/files\/uploads\/([a-f0-9-]+)$/.exec(url.pathname);
        if (uploadRoute) {
          if (method === "DELETE") { await db.call("service_cancel_upload", [authenticated.session.id, uploadRoute[1]]); return json(res, { ok: true }); }
          if (method === "PUT") {
            const upload = await db.call<{ id: string; size: number; state: string; fileId: string }>("service_upload", [authenticated.session.id, uploadRoute[1]]);
            if (upload.state === "complete") { req.resume(); return json(res, { id: upload.fileId }); }
            if (upload.state !== "uploading") throw new AppError("conflict", 409);
            const existing = await blobs.existingDigest(upload.id, upload.size);
            if (existing) req.resume();
            const digest = existing ?? await blobs.receive(req, upload.id, upload.size);
            await db.call("service_finish_upload", [authenticated.session.id, upload.id, digest]);
            return json(res, { id: upload.fileId });
          }
        }
        const downloadRoute = /^\/api\/v1\/files\/([^/]+)\/content$/.exec(url.pathname);
        if (downloadRoute && ["GET", "HEAD"].includes(method)) {
          const entry = await db.call<BlobFile>("service_download", [authenticated.session.id, downloadRoute[1], url.searchParams.get("version") ?? ""]);
          return await blobs.download(req, res, entry, url.searchParams.get("preview") === "1");
        }
        throw new AppError("notFound", 404);
      }
      if (method !== "GET" && method !== "HEAD") throw new AppError("notFound", 404);
      const interactionPage=/^\/identity\/interaction\/([A-Za-z0-9_-]+)$/.exec(url.pathname);
      if(interactionPage&&method==="GET")await identity.prepareInteraction(req,res,interactionPage[1]);
      const asset = /^\/(?:app|sdk)\.(?:js|css)$/.test(url.pathname)||/^\/chunks\/[a-zA-Z0-9_-]+\.js$/.test(url.pathname)?url.pathname.slice(1):"index.html";
      const content = await readFile(new URL(`../dist/${asset}`, import.meta.url));
      res.writeHead(200, { "content-type": asset.endsWith(".js") ? "text/javascript" : asset.endsWith(".css") ? "text/css" : "text/html", "cache-control": "no-cache" });
      res.end(method === "HEAD" ? undefined : content);
    } catch (error) {
      if (res.headersSent) { res.destroy(); return; }
      const failure = error instanceof AppError ? error : new AppError("internalError", 500, "The request could not be completed.");
      if (failure.status === 413) res.setHeader("connection", "close");
      if (failure.status === 401 && req.url?.startsWith("/dav/")) res.setHeader("www-authenticate", 'Basic realm="Dar", charset="UTF-8"');
      json(res, { error: { code: failure.code, message: failure.message } }, failure.status);
    }
  };
  const inflight = new Set<Promise<void>>();
  const server = http.createServer((req, res) => {
    const work = handle(req, res).catch(() => { res.destroy(); });
    inflight.add(work); void work.finally(() => inflight.delete(work));
  });
  server.requestTimeout = 60_000;
  server.headersTimeout = 15_000;
  server.maxConnections = 256;
  let maintenance: Promise<void> | undefined;
  let stopping = false;
  const maintain = () => {
    if (maintenance || stopping) return;
    maintenance = (async () => {
      for (const [key, attempt] of attempts) if (attempt.until <= Date.now()) attempts.delete(key);
      await reconcileStorage(db, blobs);
      await deliverReminders(db);
      await identity.maintain();
    })().catch(() => { console.warn("Storage maintenance will retry."); }).finally(() => { maintenance = undefined; });
  };
  const timer = setInterval(maintain, 60_000);
  disposeResources = async () => {
    stopping = true; clearInterval(timer);
    try {
      await new Promise<void>(resolve => { server.close(() => resolve()); server.closeAllConnections(); });
      await Promise.allSettled([...inflight]);
      await maintenance;
    } finally { attempts.clear();await newsRuntime?.close();await photosRuntime?.close();await agentRuntime?.close();await appRuntime?.close(); await releaseRoot(); }
  };
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(config.port, config.host, resolve); });
  return { server, ctx, close: () => ctx.fiber.dispose() };
  } catch (error) {await newsRuntime?.close();await photosRuntime?.close();await agentRuntime?.close();await appRuntime?.close();if (ownedContext) await ownedContext.fiber.dispose(); else await releaseRoot(); throw error; }
}
