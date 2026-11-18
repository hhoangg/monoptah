// C-7: asks the desktop to raise this device from read to control. The desktop owner still has to confirm
// (R-15), so a successful request only changes the copy to "Requested, confirm on the desktop".
// "Requested" comes from the shared grants query (this device's grant has `requestedPermission: control`), so the
// header chip and the composer agree, and a denied or cleared request returns to idle on its own.
import { useCallback, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { desktopGrantsQuery, setRequestedPermission } from "@/features/desktops/api";
import { useThisDevice } from "@/features/desktops/thisDevice";
import { useLinkState } from "@/link";

export type RequestControlState = "idle" | "pending" | "requested" | "error";

export interface RequestControl {
  state: RequestControlState;
  request: () => void;
  /** False until this device is known: the control is disabled instead of failing on tap. Undefined means ready. */
  ready?: boolean;
}

export function useRequestControl(desktopId: string): RequestControl {
  const queryClient = useQueryClient();
  const device = useThisDevice();
  const deviceId = device?.deviceId ?? null;
  const readOnly = useLinkState((state) => state.permission === "read");
  const grants = useQuery({ ...desktopGrantsQuery(desktopId), enabled: readOnly && deviceId !== null });
  const [local, setLocal] = useState<"idle" | "pending" | "error" | "sent">("idle");
  const request = useCallback(() => {
    if (!deviceId) return;
    setLocal("pending");
    setRequestedPermission(queryClient, desktopId, deviceId, "control").then(
      () => setLocal("sent"),
      () => setLocal("error"),
    );
  }, [queryClient, desktopId, deviceId]);

  const grant = deviceId ? grants.data?.grants.find((candidate) => candidate.deviceId === deviceId) : undefined;
  // The grants list is the truth once it is loaded; before that (or when it cannot be read) the answer of our own request stands.
  const requested = grant ? grant.requestedPermission === "control" : local === "sent";
  const state: RequestControlState = local === "pending" ? "pending" : requested ? "requested" : local === "error" ? "error" : "idle";
  return { state, request, ready: deviceId !== null };
}
