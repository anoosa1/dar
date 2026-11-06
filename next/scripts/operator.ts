import { readFile } from "node:fs/promises";
import { database } from "../runtime/database.ts";
import { backup, restore, exportFiles } from "../runtime/backup.ts";
import { hashPassword } from "../runtime/auth/password.ts";
function required(name: string) { const value = process.env[name]; if (!value) throw new Error(`${name} is required.`); return value; }
const db = await database(required("DAR_DATABASE_URL"), required("DAR_DATABASE_NAME"), required("DAR_DATABASE_TOKEN_FILE"));
const [command, argument, filename] = process.argv.slice(2);
if (command === "backup" && argument) console.info(await backup(db, required("DAR_ROOT"), argument));
else if (command === "restore" && argument) console.info(await restore(db, argument, required("DAR_ROOT")));
else if (command === "export-files" && argument && filename) console.info(await exportFiles(db, required("DAR_ROOT"), argument, filename));
else if (command === "release-backup" && argument) { await db.call("service_release_backup", [argument]); console.info("Backup pins released."); }
else if (["enable-user", "disable-user"].includes(command) && argument) {
  const user = await db.call<{ id: string } | null>("service_account", [argument.toLowerCase()]);
  if (!user) throw new Error("Account not found.");
  await db.call("service_set_enabled", [user.id, command === "enable-user"]);
  console.info("Account status updated.");
}
else if (command === "recover-password" && argument && filename) {
  const user = await db.call<{ id: string; verifier: string } | null>("service_account", [argument.toLowerCase()]);
  if (!user) throw new Error("Account not found.");
  const password = (await readFile(filename, "utf8")).replace(/\r?\n$/, "");
  await db.call("service_change_password", [user.id, await hashPassword(password), user.verifier]);
  console.info("Password changed; all browser sessions revoked. Securely remove the password file.");
} else throw new Error("Usage: operator.ts backup|restore <directory>, export-files <username> <directory>, release-backup <id>, enable-user|disable-user <username>, or recover-password <username> <protected-password-file>");
