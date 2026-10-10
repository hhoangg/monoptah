import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import {
  getHarnessAvailabilitySnapshot,
  hasProbedHarnessAvailability,
  isHarnessAvailable,
  subscribeHarnessAvailability,
} from "../../../integrations/harness/core/availabilityState";
import {
  loadProviderSurface,
  subscribeProviderSurface,
} from "../../providers/model/providerSurface";
import {
  moveHighlight,
  newSessionProviders,
} from "../model/newSessionProviderPrompt";
import {
  projectProvidersRevision,
  subscribeProjectProviders,
} from "../model/projectProviders";
import { HARNESSES, HARNESS_TITLE, type HarnessId } from "../model/session";
import { Modal } from "../../../shared/ui/Modal";
import { HarnessIcon } from "./HarnessIcon";

type Props = {
  /** Project cwd, used to respect that project's hidden providers. */
  cwd: string;
  /** Provider highlighted when the dialog opens - the current default. */
  defaultHarness: HarnessId;
  onPick: (harness: HarnessId) => void;
  onClose: () => void;
};

export function NewSessionProviderDialog({
  cwd,
  defaultHarness,
  onPick,
  onClose,
}: Props) {
  const availabilityVersion = useSyncExternalStore(
    subscribeHarnessAvailability,
    getHarnessAvailabilitySnapshot,
    getHarnessAvailabilitySnapshot,
  );
  const projectVersion = useSyncExternalStore(
    subscribeProjectProviders,
    projectProvidersRevision,
    projectProvidersRevision,
  );
  const surfaceVersion = useSyncExternalStore(
    subscribeProviderSurface,
    () => HARNESSES.map(loadProviderSurface).join(","),
    () => "",
  );
  const providers = useMemo(() => {
    void availabilityVersion;
    void projectVersion;
    return newSessionProviders(
      cwd,
      isHarnessAvailable,
      hasProbedHarnessAvailability(),
    );
  }, [cwd, availabilityVersion, projectVersion]);

  const [active, setActive] = useState(() =>
    Math.max(0, providers.indexOf(defaultHarness)),
  );
  const current = Math.min(active, Math.max(0, providers.length - 1));

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.isComposing || event.defaultPrevented) return;
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const direction = event.key === "ArrowDown" ? 1 : -1;
        setActive(moveHighlight(current, direction, providers.length));
      } else if (event.key === "Enter") {
        const target = providers[current];
        if (!target) return;
        event.preventDefault();
        onPick(target);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [current, providers, onPick]);

  return (
    <Modal
      onClose={onClose}
      title="Choose a provider"
      description="For this session only"
      size="sm"
    >
      <div
        role="listbox"
        aria-label="Provider"
        className="flex flex-col gap-0.5 p-2 text-[12px]"
        data-surface-version={surfaceVersion}
      >
        <p className="px-2.5 pb-1 text-[11px] text-content/55">
          The default provider opens as a terminal session, which can't switch
          providers afterwards.
        </p>
        {providers.map((id, index) => {
          const surface = loadProviderSurface(id);
          return (
            <button
              key={id}
              type="button"
              role="option"
              aria-selected={index === current}
              onClick={() => onPick(id)}
              onMouseEnter={() => setActive(index)}
              className={`flex items-center gap-2 rounded-md px-2.5 py-2 text-left active:scale-[0.99] ${
                index === current ? "bg-content/10" : "hover:bg-content/8"
              }`}
            >
              <HarnessIcon harness={id} className="size-4" />
              <span className="min-w-0 flex-1 truncate">
                {HARNESS_TITLE[id]}
              </span>
              <span className="rounded bg-content/10 px-1.5 py-0.5 text-[10px] text-content/60">
                {surface === "tui" ? "TUI" : "Chat"}
              </span>
            </button>
          );
        })}
      </div>
    </Modal>
  );
}
