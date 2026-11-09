import { startServer } from "./server.ts";
function required(name: string) {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required.`);
  return value;
}
const runtime = await startServer({
  host: process.env.DAR_HOST ?? "127.0.0.1", port: Number(process.env.DAR_PORT ?? 4311),
  origin: required("DAR_ORIGIN"), root: required("DAR_ROOT"), databaseUrl: required("DAR_DATABASE_URL"),
  databaseName: required("DAR_DATABASE_NAME"), serviceTokenFile: required("DAR_DATABASE_TOKEN_FILE"),
  databasePublicUrl: process.env.DAR_DATABASE_PUBLIC_URL,
  appsFile:process.env.DAR_APPS_FILE,
  oidcFile: process.env.DAR_OIDC_FILE,
  bootstrapTokenFile: required("DAR_BOOTSTRAP_TOKEN_FILE"), development: process.env.DAR_DEVELOPMENT === "1",
});
for (const signal of ["SIGINT", "SIGTERM"] as const) process.once(signal, () => { void runtime.close(); });
console.info("Dar auth/Files runtime listening.");
