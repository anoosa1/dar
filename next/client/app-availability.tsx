import {sharedRequest} from "./shared-request.ts";
import type {AppSettings} from "../shared/app-settings.ts";
const appSettings=sharedRequest<AppSettings>("/api/v1/appSettings",["dar:apps"]);
export function useAppSettings(){const {value,error}=appSettings.use();return {settings:value,error};}
