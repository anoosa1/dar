import type { IncomingMessage, ServerResponse } from "node:http";
import { Provider, errors, type Configuration, type KoaContextWithOIDC } from "oidc-provider";
import type { Database } from "../database.ts";
import type { Authenticated, authService } from "../auth/service.ts";
import { AppError } from "../errors.ts";
import { providerStore, providerContext } from "./store.ts";
import { providerKeys } from "./keys.ts";
import type { Document } from "./scim-model.ts";

const escape=(s:unknown)=>String(s).replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"})[c]!);
const page=(title:string,content:string)=>`<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><link rel="stylesheet" href="/app.css"><title>${escape(title)} · Dar</title></head><body><main class="identity-protocol"><a class="brand" href="/">Dar</a><h1>${escape(title)}</h1>${content}</main></body></html>`;
function formPolicy(ctx:{set(name:string,value:string):unknown},...destinations:unknown[]){const origins=destinations.filter((d):d is string=>typeof d==="string").map(d=>new URL(d).origin);ctx.set("Content-Security-Policy",`default-src 'none'; script-src 'self'; style-src 'self'; form-action 'self' ${[...new Set(origins)].join(" ")}; base-uri 'none'; frame-ancestors 'none'`);}
export const permissionNames:Record<string,string>={openid:"Identify your account",profile:"Read your name and username",email:"Read your email address",groups:"Read your directory group IDs",offline_access:"Keep access with a refresh token while your Dar session remains active"};

export async function identityProvider(db:Database,auth:Awaited<ReturnType<typeof authService>>,directory:string,origin:string,development:boolean){
  const store=await providerStore(db,directory),keys=await providerKeys(directory);
  function create(){
    const state=keys.current();
    const configuration:Configuration={
      adapter:store.Adapter,clients:[],jwks:{keys:[state.active,...state.retired.filter(k=>k.retireAt>Date.now()).map(k=>k.jwk)]},
      cookies:{keys:[state.cookieKey],names:{session:"dar_oidc",interaction:"dar_oidc_interaction",resume:"dar_oidc_resume"},long:{httpOnly:true,secure:!development,sameSite:"lax",signed:true},short:{httpOnly:true,secure:!development,sameSite:"lax",signed:true}},
      responseTypes:["code"],scopes:Object.keys(permissionNames),subjectTypes:["public"],
      claims:{openid:["sub"],profile:["name","preferred_username","given_name","family_name"],email:["email","email_verified"],groups:["groups"]},
      pkce:{required:()=>true},clientAuthMethods:["none","client_secret_basic"],
      enabledJWA:{idTokenSigningAlgValues:["RS256"]},
      features:{devInteractions:{enabled:false},revocation:{enabled:true},introspection:{enabled:true},rpInitiatedLogout:{enabled:true,logoutSource:async(ctx,form)=>{formPolicy(ctx,ctx.oidc.params?.post_logout_redirect_uri);ctx.type="html";ctx.body=page("Sign out of connected applications?",`<p>This ends the provider session and its application grants. Your Dar browser session stays signed in.</p>${form}<div class="dialog-actions"><a class="secondary-button" href="/settings">Cancel</a><button class="primary-button" type="submit" form="op.logoutForm" name="logout" value="yes">Sign out</button></div>`);},postLogoutSuccessSource:async ctx=>{ctx.type="html";ctx.body=page("Signed out","<p>The provider session has ended.</p><a href=\"/\">Return to Dar</a>");}},registration:{enabled:false},registrationManagement:{enabled:false},pushedAuthorizationRequests:{enabled:false},dPoP:{enabled:false},resourceIndicators:{enabled:false}},
      ttl:{AuthorizationCode:60,AccessToken:300,IdToken:300,RefreshToken:7*86400,Grant:7*86400,Session:7*86400,Interaction:600},
      expiresWithSession:()=>true,rotateRefreshToken:true,
      interactions:{url:(_ctx,interaction)=>`${origin}/identity/interaction/${interaction.uid}`},
      clientBasedCORS:(_ctx,requestOrigin,client)=>client.tokenEndpointAuthMethod==="none"&&!!client.redirectUris?.some(uri=>new URL(uri).origin===requestOrigin),
      findAccount:async(_ctx,id)=>{
        const account=await store.call<Document|null>("account",{id});if(!account?.active)return undefined;
        return {accountId:id,claims:async()=>{const emails=account.emails as Document[]|undefined,email=emails?.find(e=>e.primary)??emails?.[0],name=account.name as Document|undefined;return {sub:id,name:account.displayName,preferred_username:account.userName,given_name:name?.givenName,family_name:name?.familyName,...(email?{email:email.value,email_verified:false}:{}),groups:((account.groups??[]) as Document[]).map(g=>g.value)};}};
      },
      renderError:async(ctx,out)=>{ctx.type="html";ctx.body=page("Unable to continue",`<p>${escape(out.error_description??out.error??"The authorization request failed.")}</p><a href="/">Return to Dar</a>`);}
    };
    const instance=new Provider(`${origin}/oidc`,configuration);
    instance.use(async(ctx,next)=>{
      await next();
      // The library also uses form_post internally for account switching and anonymous logout.
      // Keep those automatic handoffs on the same self-hosted script as external form_post responses.
      if(typeof ctx.body==="string"&&ctx.body.includes("<title>Submitting Callback</title>")){
        const form=/<form method="post" action="([^"]+)">([\s\S]*?)<\/form>/.exec(ctx.body);
        if(!form)throw new Error("Unexpected provider form.");
        ctx.body=page("Continue sign-in",`<form id="oidc-response" method="post" action="${form[1]}">${form[2]}<button class="primary-button" type="submit">Continue</button></form><script src="/oidc-form.js" defer></script>`);
        formPolicy(ctx,form[1],(ctx as KoaContextWithOIDC).oidc.session?.state?.postLogoutRedirectUri);
      }
    });
    // TLS termination is accepted only after validateTransport checks the local proxy and canonical Host.
    instance.proxy=!development;
    instance.on("server_error",()=>console.warn("An identity provider request failed."));
    return instance;
  }
  let provider=create(),callback=provider.callback(),keyIds=keys.status().retained.map(k=>k.kid).join(",");
  function validateTransport(req:IncomingMessage){
    if(req.headers.host!==new URL(origin).host)throw new AppError("invalidHost",400);
    if(!development){if(!["127.0.0.1","::1","::ffff:127.0.0.1"].includes(req.socket.remoteAddress??"")||req.headers["x-forwarded-proto"]!=="https"||(req.headers["x-forwarded-host"]&&req.headers["x-forwarded-host"]!==new URL(origin).host))throw new AppError("untrustedProxy",400,"OIDC requires the configured local HTTPS reverse proxy.");delete req.headers["x-forwarded-for"];}
  }
  async function interaction(req:IncomingMessage,res:ServerResponse,uid:string){
    validateTransport(req);
    let detail;try{detail=await provider.interactionDetails(req,res);}catch(error){if(error instanceof errors.SessionNotFound)throw new AppError("interactionExpired",410,"This sign-in request expired. Return to the application and try again.");throw error;}if(detail.uid!==uid)throw new AppError("invalidInteraction",400);
    const clientId=String(detail.params.client_id),client=await store.call<{metadata:string;enabled:boolean}|null>("client",{id:clientId});if(!client?.enabled)throw new AppError("invalidClient",400);
    return {detail,client:JSON.parse(client.metadata) as Document};
  }
  return {
    store,keys,
    async prepareInteraction(req:IncomingMessage,res:ServerResponse,uid:string){
      await providerContext.run({},async()=>{
        try{
          const {detail}=await interaction(req,res,uid);
          // Give the API its own path-scoped copy of the verified interaction cookie.
          // A global cookie would make sign-in requests in separate tabs overwrite one another.
          const context=provider.createContext(req,res);
          context.cookies.set(provider.cookieName("interaction"),detail.uid,{path:`/api/v1/identity/interactions/${uid}`,httpOnly:true,secure:!development,sameSite:"lax",signed:true,maxAge:Math.max(0,detail.exp*1000-Date.now())});
        }catch(error){if(error instanceof AppError&&[400,410].includes(error.status))return;throw error;}
      });
    },
    async handle(req:IncomingMessage,res:ServerResponse){
      validateTransport(req);
      if((req.url?.length??0)>8192)throw new AppError("requestTooLarge",413);
      res.setHeader("content-security-policy","default-src 'none'; script-src 'self'; style-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'");
      const mounted=req as IncomingMessage&{originalUrl?:string;baseUrl?:string};
      const saved=req.url;mounted.originalUrl=saved;mounted.baseUrl="/oidc";req.url=req.url?.slice("/oidc".length)||"/";
      // The pinned Koa callback returns its middleware promise. Drain that work,
      // including adapter writes, even when the client closes its socket early.
      try{await providerContext.run({},async()=>{await callback(req,res);});}finally{req.url=saved;delete mounted.originalUrl;delete mounted.baseUrl;}
    },
    async details(req:IncomingMessage,res:ServerResponse,uid:string,authenticated:Authenticated){
      return providerContext.run({nativeSessionId:authenticated.session.id,authenticatedSessionId:authenticated.session.id},async()=>{
        const {detail,client}=await interaction(req,res,uid);
        const session=await store.call<{createdAt:number}|null>("session",{id:authenticated.session.id});
        return {uid,prompt:detail.prompt.name,client:{id:client.client_id,name:client.client_name,redirectUri:detail.params.redirect_uri},permissions:String(detail.params.scope??"openid").split(" ").map(scope=>({scope,label:permissionNames[scope]??scope})),account:authenticated.user,requiresPassword:detail.prompt.name==="login"&&(!session||session.createdAt<detail.iat*1000),providerAccountId:detail.session?.accountId};
      });
    },
    async decide(req:IncomingMessage,res:ServerResponse,uid:string,authenticated:Authenticated,input:Document){
      return providerContext.run({nativeSessionId:authenticated.session.id,authenticatedSessionId:authenticated.session.id},async()=>{
        const {detail}=await interaction(req,res,uid);let result:Record<string,unknown>;
        if(input.action==="deny")result={error:"access_denied",error_description:"The user declined the request."};
        else if(input.action==="login"){
          const session=await store.call<{createdAt:number}|null>("session",{id:authenticated.session.id});
          if(!session)throw new AppError("unauthenticated",401);
          if(session.createdAt<detail.iat*1000){if(typeof input.password!=="string"||Buffer.byteLength(input.password)>1024)throw new AppError("invalidCredentials",401);await auth.confirm(authenticated,input.password);}
          result={login:{accountId:authenticated.user.id,amr:["pwd"],ts:Math.floor(Date.now()/1000),remember:true}};
        }else if(input.action==="approve"&&detail.prompt.name==="consent"){
          if(detail.session?.accountId!==authenticated.user.id)throw new AppError("accountChanged",409,"Use the account shown in the request, or switch accounts.");
          let grant=detail.grantId?await provider.Grant.find(detail.grantId):undefined;
          if(grant&&(grant.accountId!==authenticated.user.id||grant.clientId!==detail.params.client_id))throw new AppError("invalidGrant",400);
          grant??=new provider.Grant({accountId:authenticated.user.id,clientId:String(detail.params.client_id)});
          const scopes=detail.prompt.details.missingOIDCScope as string[]|undefined,claims=detail.prompt.details.missingOIDCClaims as string[]|undefined;
          if(scopes)grant.addOIDCScope(scopes.join(" "));if(claims)grant.addOIDCClaims(claims);
          result={consent:{grantId:await grant.save()}};
        }else throw new AppError("invalidInput",400);
        const redirect=await provider.interactionResult(req,res,result,{mergeWithLastSubmission:input.action==="approve"});
        if(!redirect.startsWith(`${origin}/oidc/`))throw new AppError("invalidRedirect",500);
        return {redirect};
      });
    },
    async rotate(){await keys.rotate();provider=create();callback=provider.callback();keyIds=keys.status().retained.map(k=>k.kid).join(",");return keys.status();},
    async maintain(){await store.call("cleanup");await keys.prune();const current=keys.current().retired.map(k=>k.jwk.kid).join(",");if(current!==keyIds){provider=create();callback=provider.callback();keyIds=current;}},
  };
}
export type IdentityProvider=Awaited<ReturnType<typeof identityProvider>>;
