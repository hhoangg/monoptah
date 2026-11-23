// This browser's device identity (server device id and key fingerprint), for the "This device" marker.
// Owner: WS-M (U-01). Read only: it never creates a key or registers a device (`ensureDevice` does that).
import { useQuery } from "@tanstack/react-query";
import { fingerprint } from "@/link/fingerprint";
import { getDeviceKeyInfo } from "@/link/keys";
import { desktopKeys } from "./api";

export interface ThisDevice {
  /** Server device id; null while the key exists but `POST /api/devices` has not run yet. */
  deviceId: string | null;
  /** Lowercase hex SHA-256 of the device public key. */
  fingerprint: string;
}

/** The stored identity, or null when this browser has no device key yet. Never throws. */
export async function readThisDevice(): Promise<ThisDevice | null> {
  try {
    const info = await getDeviceKeyInfo();
    if (!info) return null;
    return { deviceId: info.deviceId ?? null, fingerprint: fingerprint(info.publicKey) };
  } catch {
    return null;
  }
}

/** The query behind `useThisDevice`, for callers that must tell "still loading" (`isPending`) from "no identity" (data null). */
export function useThisDeviceQuery() {
  return useQuery({ queryKey: desktopKeys.thisDevice, queryFn: readThisDevice, staleTime: 0, gcTime: 0 });
}

export function useThisDevice(): ThisDevice | null {
  return useThisDeviceQuery().data ?? null;
}
