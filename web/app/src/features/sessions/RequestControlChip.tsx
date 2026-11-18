// The "Read only" chip of a workspace header, as a button that asks the person at the desktop for control (C-7).
// The desktop confirms the raise (R-15); the request itself is the shared `useRequestControl` hook.
import { useRequestControl } from "@/features/composer/useRequestControl";
import { Glass } from "@/ui/Glass";

export function RequestControlChip({ desktopId }: { desktopId: string }) {
  const { state, request, ready = true } = useRequestControl(desktopId);
  const locked = state === "pending" || !ready;
  return (
    <Glass className="flex min-h-9 items-center rounded-pill">
      <button
        type="button"
        data-testid="read-only-chip"
        data-state={state}
        disabled={locked}
        title={ready ? "Ask the desktop for control" : "Checking this device..."}
        onClick={request}
        className="inline-flex min-h-11 items-center rounded-pill px-3 text-meta font-medium hover:bg-selection disabled:opacity-100 pointer-fine:min-h-9"
      >
        {state === "requested" ? "Requested, confirm on the desktop" : state === "pending" ? "Requesting..." : state === "error" ? "Could not request, try again" : "Read only"}
        {state === "idle" ? <span className="sr-only">. Request control</span> : null}
        {state === "requested" ? <span className="sr-only">. Ask again</span> : null}
        {!ready ? <span className="sr-only">. Checking this device...</span> : null}
      </button>
    </Glass>
  );
}
