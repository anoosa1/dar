import {access,stat} from "node:fs/promises";
import {constants} from "node:fs";
import path from "node:path";

/** systemd does not inherit an interactive shell's PATH. Check standard system
 * profiles and the service user's installations, without scanning other homes
 * or selecting an arbitrary version from the Nix store.
 */
export function codexSearchDirectories(environmentPath:string,serviceHome:string){
  return [...new Set([
    ...environmentPath.split(path.delimiter),
    "/run/current-system/sw/bin","/usr/local/bin","/usr/bin","/bin",
    path.join(serviceHome,".nix-profile","bin"),path.join(serviceHome,".local","bin"),
  ].filter(directory=>path.isAbsolute(directory)))];
}

/** Executable profile symlinks are normal on NixOS; their targets must be files. */
export async function findCodexExecutable(directories:string[]){
  for(const directory of directories){
    if(!path.isAbsolute(directory))continue;
    const candidate=path.join(directory,"codex");
    try{await access(candidate,constants.X_OK);if((await stat(candidate)).isFile())return candidate;}catch{}
  }
  return undefined;
}
