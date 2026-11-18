// C-7: asks the desktop to raise this device from read to control. The desktop owner still has to confirm
// (R-15), so a successful request only changes the copy to "Requested, confirm on the desktop".
import { useCallback, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { setRequestedPermission } from "@/features/desktops/api";
import { useThisDevice } from "@/features/desktops/thisDevice";

export type RequestControlState = "idle" | "pending" | "requested" | "error";

export interface RequestControl {
  state: RequestControlState;
  request: () => void;
}

export function useRequestControl(desktopId: string): RequestControl {
  const queryClient = useQueryClient();
  const device = useThisDevice();
  const [state, setState] = useState<RequestControlState>("idle");
  const deviceId = device?.deviceId ?? null;
  const request = useCallback(() => {
    if (!deviceId) {
      setState("error");
      return;
    }
    setState("pending");
    setRequestedPermission(queryClient, desktopId, deviceId, "control").then(
      () => setState("requested"),
      () => setState("error"),
    );
  }, [queryClient, desktopId, deviceId]);
  return { state, request };
}
