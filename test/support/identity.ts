// Identity constants as the protocols name them, and the provider's record keys.
import { createHash } from "node:crypto";

export const USER = "urn:ietf:params:scim:schemas:core:2.0:User";
export const GROUP = "urn:ietf:params:scim:schemas:core:2.0:Group";
export const ENTERPRISE = "urn:ietf:params:scim:schemas:extension:enterprise:2.0:User";
export const PATCH = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

/** The provider stores each record under `<Model>/<SHA-256 of its secret or ID>`. */
export const modelKey = (model: string, id: string) => `${model}/${createHash("sha256").update(id).digest("hex")}`;
