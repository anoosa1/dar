import { spawnSync } from "node:child_process";
import path from "node:path";
const root = path.resolve(import.meta.dirname, "..");
const executable = process.env.DAR_SPACETIME_BIN ?? "spacetime";
const globalArgs = process.env.DAR_SPACETIME_CLI_ROOT ? ["--root-dir", process.env.DAR_SPACETIME_CLI_ROOT] : [];
for (const args of [
  [process.execPath, path.join(root, "node_modules/typescript/bin/tsc"), "-p", path.join(root, "module/tsconfig.json")],
  [executable, ...globalArgs, "build", "--module-path", path.join(root, "module")],
  [executable, ...globalArgs, "generate", "--lang", "typescript", "--js-path", path.join(root, "module/dist/bundle.js"), "--out-dir", path.join(root, "client/module_bindings"), "--import-extension", "ts", "--yes"],
]) {
  const result = spawnSync(args[0], args.slice(1), { cwd: root, stdio: "inherit" });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
