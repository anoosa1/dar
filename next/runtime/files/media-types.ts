import {isUtf8} from "node:buffer";

/** Media types by file extension, for files written outside a browser upload (Agent branches, Git pushes). */
export const extensionTypes:Record<string,string>={md:"text/markdown",markdown:"text/markdown",txt:"text/plain",csv:"text/csv",html:"text/html",css:"text/css",js:"text/javascript",mjs:"text/javascript",ts:"text/plain",tsx:"text/plain",json:"application/json",xml:"application/xml",svg:"image/svg+xml",png:"image/png",jpg:"image/jpeg",jpeg:"image/jpeg",gif:"image/gif",webp:"image/webp",pdf:"application/pdf"};
/** The type for a name, falling back to plain text for UTF-8 content (when given). */
export function mediaTypeOf(name:string,data?:Buffer){const extension=/\.([A-Za-z0-9]+)$/.exec(name)?.[1].toLowerCase();return extension&&extensionTypes[extension]||(data&&isUtf8(data)?"text/plain":"application/octet-stream");}
