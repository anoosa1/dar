import type {Ctx} from "./index.ts";
import type {AppSettings} from "../../shared/app-settings.ts";
/** Instance app availability shares the revisioned settings store and backup path. */
export function appSettings(ctx:Ctx):AppSettings {
  const row=ctx.db.pimPreferences.userId.find("instance:apps");
  return {revision:row?.revision??0,disabled:row?JSON.parse(row.value).disabled:[]};
}
export function appEnabled(ctx:Ctx,id:string){return !appSettings(ctx).disabled.includes(id);}
