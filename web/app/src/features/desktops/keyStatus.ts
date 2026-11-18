// What this device knows about a desktop's key, for the detail page. Owner: WS-M (U-01, security.md 5).
// The `fingerprint` field of the directory answer is only a claim of the server and is never shown as proof:
// the shown value is the pin of this device (read only, `getPin`), or the SHA-256 of the directory
// `publicKey` computed here, labelled as not yet verified.
import { useQuery } from "@tanstack/react-query";
import { base64UrlDecode } from "@/link/base64url";
import { fingerprint } from "@/link/fingerprint";
import { getPin } from "@/link/pins";

const PUBLIC_KEY_BYTES = 32;

export type KeyStatus =
  | { kind: "loading" }
  /** The key cannot be read from the directory answer. */
  | { kind: "invalid" }
  /** Pinned on this device and equal to the directory key. */
  | { kind: "pinned"; pinned: string }
  /** Pinned on this device, but the directory serves another key. */
  | { kind: "changed"; pinned: string; directory: string }
  /** Never pinned on this device: only the locally computed fingerprint of the directory key. */
  | { kind: "unpinned"; computed: string };

/** SHA-256 fingerprint of the directory public key, computed here; null when it is not a 32-byte key. */
export function computeDirectoryFingerprint(publicKey: string): string | null {
  try {
    const key = base64UrlDecode(publicKey);
    return key.length === PUBLIC_KEY_BYTES ? fingerprint(key) : null;
  } catch {
    return null;
  }
}

export function useKeyStatus(desktopId: string, publicKey: string): KeyStatus {
  const pin = useQuery({
    queryKey: ["desktops", "pin", desktopId],
    queryFn: async () => (await getPin(desktopId)) ?? null,
    staleTime: 0,
    gcTime: 0,
  });
  const computed = computeDirectoryFingerprint(publicKey);
  if (computed === null) return { kind: "invalid" };
  if (pin.isPending) return { kind: "loading" };
  const pinned = pin.data?.fingerprint ?? null;
  if (pinned === null) return { kind: "unpinned", computed };
  return pinned === computed ? { kind: "pinned", pinned } : { kind: "changed", pinned, directory: computed };
}
