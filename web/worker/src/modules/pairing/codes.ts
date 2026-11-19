// Pairing codes (architecture.md 6.4): unbiased CSPRNG sampling by rejection.
import { USER_CODE_ALPHABET, USER_CODE_LENGTH } from "@monoptah/protocol";

/** A uniform integer below `n` (n <= 256), by rejecting bytes above the largest multiple of `n`. */
function randomBelow(n: number): number {
  const limit = 256 - (256 % n);
  const buffer = new Uint8Array(1);
  for (;;) {
    crypto.getRandomValues(buffer);
    const byte = buffer[0] as number;
    if (byte < limit) return byte % n;
  }
}

/** 8 characters of the ambiguity-free alphabet, no dash. */
export function newUserCode(): string {
  let code = "";
  for (let i = 0; i < USER_CODE_LENGTH; i += 1) code += USER_CODE_ALPHABET.charAt(randomBelow(USER_CODE_ALPHABET.length));
  return code;
}

/** 6 decimal digits, each uniform. */
export function newConfirmationCode(): string {
  let code = "";
  for (let i = 0; i < 6; i += 1) code += String(randomBelow(10));
  return code;
}

/** `WXYZ-4829` display form of a stored user code. */
export function formatUserCode(code: string): string {
  return `${code.slice(0, 4)}-${code.slice(4)}`;
}

/** The stored form of a typed or linked code (case-insensitive, optional dashes), or null when malformed. */
export function normalizeUserCode(text: string): string | null {
  const code = text.toUpperCase().replaceAll("-", "");
  if (code.length !== USER_CODE_LENGTH) return null;
  for (const ch of code) if (!USER_CODE_ALPHABET.includes(ch)) return null;
  return code;
}
