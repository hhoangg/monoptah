import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { expandBytes, type BytesVector } from "../scripts/vectors-lib.ts";

export { expandBytes };
export type { BytesVector };

export function readText(relative: string): string {
  return readFileSync(fileURLToPath(new URL(relative, import.meta.url)), "utf8");
}

/** The committed vectors.json, parsed (not regenerated). */
export const vectors = JSON.parse(readText("../vectors.json")) as {
  constants: Record<string, unknown>;
  frames: {
    valid: { name: string; frame: BytesVector; decoded: { type: string; typeCode: number; channel: number; deviceId: string; body: BytesVector } }[];
    invalid: { name: string; error: string; frame: BytesVector }[];
  };
  plaintext: {
    valid: { name: string; plaintext: BytesVector; decoded: Record<string, unknown> & { kind: string } }[];
    invalid: { name: string; error: string; plaintext: BytesVector }[];
    split: { name: string; messageLength: number; messageFill: string; msgId: number; partLengths: number[] }[];
  };
  close: { code: number; name: string; device: { action: string; message?: string }; desktop: { action: string; message?: string } }[];
  errors: { desktop: string[]; relay: string[]; api: string[] };
};
