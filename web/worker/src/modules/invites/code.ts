// Invite codes (A-06). The code is 16 random bytes, base64url (128 bits). D1 stores `invites.code_hash`, the
// lowercase hex SHA-256 of the UTF-8 code string as submitted, which is exactly what scripts/bootstrap.mjs writes.
import { randomToken, sha256Hex } from "../../lib/crypto.ts";

export const INVITE_TTL_SECONDS = 7 * 86400;

export function newInviteCode(): string {
  return randomToken(16);
}

export function hashInviteCode(code: string): Promise<string> {
  return sha256Hex(code);
}
