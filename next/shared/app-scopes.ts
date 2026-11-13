export const passwordApps = ["files", "contacts", "calendar", "news"] as const;
export type PasswordApp = typeof passwordApps[number];
export type Permission = "read" | "write";
export const appLabels: Record<PasswordApp, string> = {files:"Files", contacts:"Contacts", calendar:"Calendar", news:"News"};

/** Legacy write scopes included read access. New space-separated grants are independent. */
export function passwordGrants(scope: string): string[] {
  if (scope === "read" || scope === "write") return scope === "write" ? ["files:read", "files:write"] : ["files:read"];
  if (/^(contacts|calendar|pim):(read|write)$/.test(scope)) {
    const [app, permission] = scope.split(":");
    return (app === "pim" ? ["contacts", "calendar"] : [app]).flatMap(a => permission === "write" ? [a+":read", a+":write"] : [a+":read"]);
  }
  // The v2 marker distinguishes independent write grants from legacy read/write.
  return scope.startsWith("v2 ") ? scope.slice(3).split(" ") : [];
}
export function validPasswordScope(scope: string): boolean {
  const grants = passwordGrants(scope);
  return grants.length > 0 && grants.length <= 8 && new Set(grants).size === grants.length &&
    grants.every(g => passwordApps.some(app => g === app+":read" || g === app+":write"));
}
export function passwordAllows(scope: string, app: string, permission: Permission): boolean {
  return passwordGrants(scope).includes(app+":"+permission);
}
export function passwordScopeLabel(scope: string): string {
  return passwordApps.flatMap(app => {
    const read = passwordAllows(scope, app, "read"), write = passwordAllows(scope, app, "write");
    return read || write ? [appLabels[app]+": "+(read && write ? "Read and write" : read ? "Read" : "Write")] : [];
  }).join(" · ");
}
