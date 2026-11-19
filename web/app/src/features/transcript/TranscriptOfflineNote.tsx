// F-42: the offline state inside the transcript area (the global banner of F-07 sits above the whole app).
// It says that what is on screen is the saved copy, and why it is not moving. A polite live region that is
// always mounted, so a change is announced. Tokens only; no layout shift when it is empty.
import { formatLastSeen, type Connectivity } from "@/features/offline/connectivity";
import { useDesktopConnectivity } from "@/features/offline/OfflineBanner";
import { Icon } from "@/ui/Icon";

const COPY: Record<Exclude<Connectivity["kind"], "online">, string> = {
  offline: "You are offline. Showing the saved conversation.",
  desktop_offline: "Your desktop is offline. Showing the saved conversation.",
  reconnecting: "Reconnecting to your desktop. Showing the saved conversation.",
};

const CONNECTING_COPY = "Connecting to your desktop. Showing the saved conversation.";

/** The pure view, for tests and previews. */
export function TranscriptOfflineView({ connectivity }: { connectivity: Connectivity }) {
  const kind = connectivity.kind;
  const showing = kind !== "online";
  return (
    <div role="status" aria-live="polite" data-testid="transcript-offline">
      {showing ? (
        <p data-kind={connectivity.kind} className="mx-auto flex w-full max-w-[768px] items-center justify-center gap-2 px-5 py-2 text-center text-small text-muted-text">
          <Icon name={connectivity.kind === "reconnecting" ? "spinner" : "computerOff"} size={16} className="shrink-0" />
          <span>
            {connectivity.connecting ? CONNECTING_COPY : COPY[kind]}
            {connectivity.lastSeenAt !== null ? ` Last seen ${formatLastSeen(connectivity.lastSeenAt)}.` : ""}
          </span>
        </p>
      ) : null}
    </div>
  );
}

export function TranscriptOfflineNote({ desktopId }: { desktopId: string }) {
  return <TranscriptOfflineView connectivity={useDesktopConnectivity(desktopId)} />;
}
