import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
} from "react";
import { ask as askDialog } from "@tauri-apps/plugin-dialog";
import { OverlayNav } from "../../../app/shell/TitleBar";
import { WindowControls } from "../../../app/shell/WindowControls";
import { IS_MAC } from "../../../platform/tauri/platform";
import { projectName } from "../../../shared/lib/paths";
import { isRemoteProjectPath } from "../../projects/model/recents";
import { defaultSessionChoice } from "../../sessions/model/models";
import { newSessionProviders } from "../../sessions/model/newSessionProviderPrompt";
import { HARNESS_TITLE } from "../../sessions/model/session";
import type { HarnessId } from "../../sessions/model/session";
import { useLockOverscroll } from "../../../shared/hooks/useLockOverscroll";
import {
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  Loader,
  MoreHorizontal,
  RefreshCw,
  Trash2,
  Sparkles,
  Trophy,
  Users,
} from "../../../shared/ui/icons";
import { lazySurface } from "../../../shared/ui/lazySurface";
import { Popover } from "../../../shared/ui/Popover";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { Segmented, Select, Toggle } from "../../settings/ui/primitives";
import { useContributors } from "../hooks/useContributors";
import type { AskAgentRequest } from "../model/askAgent";
import { CONFIG_PATH } from "../model/config";
import { PERIOD_KINDS } from "../model/period";
import type { SnapshotScope } from "../model/snapshot";
import type { ContributorsConfig } from "../model/types";
import { AskAgentDialog } from "./AskAgentDialog";
import type { AskProvider } from "./AskAgentDialog";
import { GhNotice } from "./GhNotice";
import type { IdentitiesTabProps } from "./IdentitiesTab";
import type { LeaderboardTabProps } from "./LeaderboardTab";
import { NotSyncedPanel, SyncedBar } from "./SyncPanel";
import { StatsTab } from "./StatsTab";

// Loaded on first use: the tabs are separate files so each can grow without
// weighing down the page shell.
const LeaderboardTab = lazySurface<LeaderboardTabProps>(() =>
  import("./LeaderboardTab").then((m) => ({ default: m.LeaderboardTab })),
);
const IdentitiesTab = lazySurface<IdentitiesTabProps>(() =>
  import("./IdentitiesTab").then((m) => ({ default: m.IdentitiesTab })),
);

const PERIOD_ARROW =
  "flex size-7 items-center justify-center rounded-md text-content/60 hover:bg-content/8 hover:text-content disabled:pointer-events-none disabled:opacity-35";

type TabId = "stats" | "leaderboard" | "identities";

const TABS: { id: TabId; label: string }[] = [
  { id: "stats", label: "Stats" },
  { id: "leaderboard", label: "Leaderboard" },
  { id: "identities", label: "Identities" },
];

type Props = {
  /** Path of the local project whose history is shown. */
  project: string;
  besideRail?: boolean;
  compactRail?: boolean;
  onClose: () => void;
  onToggleSidebar?: () => void;
  /** Reports the scope when an Ask button is pressed. With `onStartAgentSession` the page also opens its own dialog. */
  onAskAgent?: (scope: SnapshotScope) => void;
  /**
   * Starts the agent session for a finished Ask Agent dialog. A rejection is
   * shown in the dialog; resolving closes it. The Ask buttons are hidden
   * unless this or `onAskAgent` is given.
   */
  onStartAgentSession?: (request: AskAgentRequest) => Promise<void>;
};

type AskState = {
  scope: SnapshotScope;
  providers: AskProvider[];
  defaultProvider: HarnessId | undefined;
};

function TabButton({
  id,
  label,
  badge,
  selected,
  onSelect,
}: {
  id: TabId;
  label: string;
  badge?: string;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      role="tab"
      id={`contributors-tab-${id}`}
      aria-selected={selected}
      aria-controls="contributors-panel"
      tabIndex={selected ? 0 : -1}
      onClick={onSelect}
      className={`relative flex h-9 items-center px-3 text-[12px] leading-none outline-none focus-visible:outline-2 focus-visible:outline-accent ${
        selected ? "text-content" : "text-content/50 hover:text-content"
      }`}
    >
      {label}
      {badge ? (
        <span className="ml-1.5 rounded-full bg-content/10 px-1.5 py-0.5 text-[10px] tabular-nums text-content/60">
          {badge}
        </span>
      ) : null}
      {selected ? (
        <span className="absolute inset-x-0 bottom-0 h-0.5 bg-content" />
      ) : null}
    </button>
  );
}

function Alert({
  children,
  onRetry,
  retrying = false,
  actionLabel = "Retry",
}: {
  children: string;
  onRetry?: () => void;
  retrying?: boolean;
  actionLabel?: string;
}) {
  return (
    <div
      role="alert"
      className="mb-4 flex items-start gap-2.5 rounded-xl border border-red-400/30 bg-red-400/5 px-3.5 py-2.5 text-[12px] text-red-400"
    >
      <CircleAlert className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.75} />
      <p className="min-w-0 flex-1 break-words leading-relaxed">{children}</p>
      {onRetry ? (
        <SecondaryButton onClick={onRetry} disabled={retrying}>
          <RefreshCw className="size-3" strokeWidth={1.75} />
          {actionLabel}
        </SecondaryButton>
      ) : null}
    </div>
  );
}

export function ContributorsView({
  project,
  besideRail = false,
  compactRail = false,
  onClose,
  onToggleSidebar,
  onAskAgent,
  onStartAgentSession,
}: Props) {
  // A remote project has no local repository; the hook stays idle for it.
  const remote = isRemoteProjectPath(project);
  const data = useContributors(remote ? "" : project);
  const lockOverscroll = useLockOverscroll<HTMLDivElement>();
  const [tab, setTab] = useState<TabId>("stats");
  // The formula sidebar slides in over the page body, below the header.
  const [sidebarHost, setSidebarHost] = useState<HTMLDivElement | null>(null);
  const [ask, setAsk] = useState<AskState | null>(null);
  const [menu, setMenu] = useState(false);
  const menuButton = useRef<HTMLButtonElement | null>(null);
  const { model, refreshing, sync } = data;
  const status = remote ? "remote" : data.status;
  // The config file exists but cannot be read: every write is blocked.
  const locked = !remote && !!data.configError;
  // A refused or failed save is already shown (the banner or the save alert),
  // so the tabs need not handle the rejection themselves.
  const saveConfig = (next: ContributorsConfig) =>
    data.saveConfig(next).catch(() => undefined);

  // The control that opened the dialog, so closing it hands focus back (the
  // dialog's own close would leave it on the page body).
  const askOpener = useRef<HTMLElement | null>(null);
  const askWasOpen = useRef(false);
  const onClickCapture = (event: MouseEvent) => {
    if (ask || !(event.target instanceof Element)) return;
    askOpener.current = event.target.closest("button");
  };
  useEffect(() => {
    if (askWasOpen.current && !ask) {
      const opener = askOpener.current;
      if (opener?.isConnected) opener.focus();
    }
    askWasOpen.current = ask !== null;
  }, [ask]);

  // Like Settings: a plain Escape closes the page. It runs in the bubble phase,
  // after the dialog, popovers, the formula sidebar and name inputs have had
  // their turn, and claiming the key keeps App from stopping a running agent
  // turn in a tab behind this page.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      const target = event.target;
      // Leaving a text field is the first step; the page closes on the next.
      if (
        target instanceof HTMLElement &&
        (target.isContentEditable ||
          target instanceof HTMLInputElement ||
          target instanceof HTMLTextAreaElement)
      ) {
        target.blur();
        return;
      }
      onCloseRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Asking needs the data on screen; a click before it arrives must not open
  // the dialog later as a side effect.
  const askReady = status === "ready" && model != null;
  // Providers are read when the dialog opens so a freshly installed one shows up.
  const openAsk =
    onStartAgentSession || onAskAgent
      ? (scope: SnapshotScope) => {
          if (!askReady) return;
          onAskAgent?.(scope);
          if (!onStartAgentSession) return;
          setAsk({
            scope,
            providers: newSessionProviders(project).map((id) => ({
              id,
              label: HARNESS_TITLE[id],
            })),
            defaultProvider: defaultSessionChoice(project).harness,
          });
        }
      : undefined;

  const branchOptions = useMemo(() => {
    const names = new Set(data.branches);
    if (data.branch) names.add(data.branch);
    return Array.from(names, (name) => ({ value: name, label: name }));
  }, [data.branches, data.branch]);

  const unmapped = model?.unmapped.length ?? 0;

  // A native sheet: `window.confirm` is swallowed when a macOS menu accelerator fires.
  const clearSynced = async () => {
    setMenu(false);
    const confirmed = await askDialog(
      "Delete the synced Contributors data of this repository? You can sync it again at any time.",
      { title: "Monoptah", kind: "warning" },
    );
    if (confirmed) await data.clearSyncedData();
  };
  // "Last 7 days" reads wrong in "No data for ... yet"; the others name themselves.
  const periodName =
    data.period.kind === "7d" ? "the last 7 days" : data.period.label;

  const onTabKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const index = TABS.findIndex((t) => t.id === tab);
    let target: number;
    if (e.key === "ArrowRight") target = (index + 1) % TABS.length;
    else if (e.key === "ArrowLeft")
      target = (index - 1 + TABS.length) % TABS.length;
    else if (e.key === "Home") target = 0;
    else if (e.key === "End") target = TABS.length - 1;
    else return;
    e.preventDefault();
    const next = TABS[target];
    setTab(next.id);
    document.getElementById(`contributors-tab-${next.id}`)?.focus();
  };

  return (
    <div
      role="region"
      aria-label="Contributors"
      data-app-contributors
      onClickCapture={onClickCapture}
      className="flex min-h-0 min-w-0 flex-1 flex-col text-content"
    >
      <div
        className="flex h-10 shrink-0 select-none items-center border-b border-stroke"
        data-tauri-drag-region="deep"
      >
        {IS_MAC && compactRail ? <div className="w-4 shrink-0" /> : null}
        {IS_MAC && !besideRail ? <div className="w-[78px] shrink-0" /> : null}
        {besideRail ? null : (
          <OverlayNav onBack={onClose} onToggleSidebar={onToggleSidebar} />
        )}
        <div className="flex min-w-0 flex-1 items-center gap-2 px-3 text-[13px]">
          <Trophy
            className="size-3.5 shrink-0 text-content/45"
            strokeWidth={1.75}
          />
          <span className="min-w-0 truncate text-content/45">
            {projectName(project)}
          </span>
          <span aria-hidden className="shrink-0 text-content/25">
            /
          </span>
          <span className="min-w-0 truncate text-content">Contributors</span>
        </div>
        {IS_MAC ? null : <WindowControls />}
      </div>

      <div className="relative flex min-h-0 flex-1 flex-col">
        <div
          ref={lockOverscroll}
          className="@container/contrib min-h-0 flex-1 overflow-y-auto overscroll-none"
        >
          <div className="mx-auto w-full max-w-5xl px-5 py-6 pb-16 @min-[560px]/contrib:px-8">
            {remote ? (
              <div className="flex flex-col items-center gap-2 rounded-xl border border-stroke px-4 py-8 text-center">
                <Users className="size-5 text-content/35" strokeWidth={1.75} />
                <p className="text-[13px] font-medium">
                  Contributors works on local projects only
                </p>
                <p className="text-[12px] text-content/50">
                  Open this project on this computer to see its history.
                </p>
              </div>
            ) : null}
            <div
              className={`mb-4 flex-wrap items-center gap-2.5 ${remote ? "hidden" : "flex"}`}
            >
              <h1 className="mr-auto text-[20px] font-semibold leading-tight">
                Contributors
              </h1>
              {branchOptions.length > 0 ? (
                <Select
                  label="Branch"
                  value={data.branch}
                  options={branchOptions}
                  onChange={data.setBranch}
                />
              ) : null}
              <Segmented
                label="Period"
                value={data.period.kind}
                options={PERIOD_KINDS}
                onChange={data.setKind}
              />
              {data.period.kind !== "7d" ? (
                <div
                  role="group"
                  aria-label="Period navigation"
                  className="flex items-center gap-0.5 text-[12px]"
                >
                  <button
                    type="button"
                    aria-label="Previous period"
                    onClick={data.goPrevious}
                    className={PERIOD_ARROW}
                  >
                    <ChevronLeft className="size-3.5" strokeWidth={1.75} />
                  </button>
                  <span
                    aria-live="polite"
                    className="min-w-[4.5rem] text-center tabular-nums"
                  >
                    {data.period.label}
                  </span>
                  <button
                    type="button"
                    aria-label="Next period"
                    disabled={!data.hasNext}
                    onClick={data.goNext}
                    className={PERIOD_ARROW}
                  >
                    <ChevronRight className="size-3.5" strokeWidth={1.75} />
                  </button>
                </div>
              ) : null}
              <label className="flex items-center gap-2 text-[12px] text-content/60">
                Merge identities
                <Toggle
                  label="Merge identities"
                  on={data.merge}
                  onChange={data.setMerge}
                />
              </label>
              <button
                type="button"
                aria-label="Refresh"
                title="Reload synced data"
                disabled={status === "loading" || refreshing}
                onClick={() => void data.refresh(true)}
                className="rounded-md p-1.5 text-content/40 hover:bg-content/8 hover:text-content disabled:opacity-30"
              >
                {refreshing ? (
                  <Loader
                    className="size-3.5 animate-spin"
                    strokeWidth={1.75}
                  />
                ) : (
                  <RefreshCw className="size-3.5" strokeWidth={1.75} />
                )}
              </button>
              <button
                ref={menuButton}
                type="button"
                aria-label="More"
                aria-haspopup="menu"
                aria-expanded={menu}
                onClick={() => setMenu((open) => !open)}
                className="rounded-md p-1.5 text-content/40 hover:bg-content/8 hover:text-content aria-expanded:bg-content/8 aria-expanded:text-content"
              >
                <MoreHorizontal className="size-3.5" strokeWidth={1.75} />
              </button>
              {menu ? (
                <Popover
                  anchor={menuButton}
                  align="end"
                  width={200}
                  onDismiss={() => setMenu(false)}
                  role="menu"
                  aria-label="Contributors options"
                  className="p-1"
                >
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => void clearSynced()}
                    className="flex w-full items-center gap-2.5 rounded-lg px-2.5 py-1.5 text-left text-[13px] text-content/85 hover:bg-content/8"
                  >
                    <Trash2 className="size-3.5 shrink-0" strokeWidth={1.75} />
                    Clear synced data
                  </button>
                </Popover>
              ) : null}
              {openAsk ? (
                <button
                  type="button"
                  disabled={!askReady}
                  onClick={() => openAsk({ kind: "team" })}
                  className="flex items-center gap-1.5 rounded-md bg-accent px-3 py-1 text-[12px] font-medium text-white hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-default disabled:opacity-40"
                >
                  <Sparkles className="size-3.5" strokeWidth={1.75} />
                  Ask Agent
                </button>
              ) : null}
            </div>

            {data.saveError ? (
              <Alert>{`Could not save ${CONFIG_PATH}: ${data.saveError}`}</Alert>
            ) : null}
            {sync.jobError ? (
              <Alert>{`The sync stopped: ${sync.jobError}`}</Alert>
            ) : null}
            {sync.actionError ? <Alert>{sync.actionError}</Alert> : null}

            {status === "loading" ? (
              <div
                role="status"
                className="flex items-center gap-2 py-10 text-[12px] text-content/50"
              >
                <Loader className="size-4 animate-spin" strokeWidth={1.75} />
                Reading the git history...
              </div>
            ) : null}

            {status === "error" ? (
              <Alert
                onRetry={() => void data.refresh(true)}
                retrying={refreshing}
              >
                {data.error ?? "Could not load contributors."}
              </Alert>
            ) : null}

            {status === "ready" && !model && sync.state === "notSynced" ? (
              <NotSyncedPanel
                label={periodName}
                estimate={sync.estimate}
                job={sync.job}
                onSync={() => void data.startSync()}
                onCancel={() => void data.cancelSync()}
              />
            ) : null}

            {status === "ready" && model ? (
              <>
                {data.error ? (
                  <Alert
                    onRetry={() => void data.refresh(true)}
                    retrying={refreshing}
                  >
                    {`Could not refresh: ${data.error}`}
                  </Alert>
                ) : null}
                {locked ? (
                  <Alert
                    actionLabel="Reload"
                    onRetry={() => void data.refresh(true)}
                    retrying={refreshing}
                  >
                    {`${CONFIG_PATH} could not be read (${data.configError}). Fix the file in your repo, then reload. Editing is disabled until then.`}
                  </Alert>
                ) : null}
                {data.stale ? null : (
                  <SyncedBar
                    syncedAt={sync.syncedAt}
                    now={data.now}
                    job={sync.job}
                    onSync={() => void data.startSync()}
                    onCancel={() => void data.cancelSync()}
                  />
                )}
                {sync.ghIncomplete ||
                !model.raw.ghAvailable ||
                model.raw.ghNote?.trim() ? (
                  <GhNotice
                    available={model.raw.ghAvailable}
                    incomplete={sync.ghIncomplete}
                    note={model.raw.ghNote}
                    retrying={sync.job !== null}
                    onRetry={() => void data.retryGithub()}
                  />
                ) : null}
                {model.raw.truncated ? (
                  <p className="mb-4 text-[12px] text-amber-700 dark:text-amber-400">
                    The history was too large to read in full, so some numbers
                    may be partial.
                  </p>
                ) : null}

                <div
                  role="tablist"
                  aria-label="Contributors sections"
                  onKeyDown={onTabKey}
                  className="mb-4 flex gap-1 border-b border-stroke"
                >
                  {TABS.map((t) => (
                    <TabButton
                      key={t.id}
                      id={t.id}
                      label={t.label}
                      badge={
                        t.id === "identities" && unmapped > 0
                          ? `${unmapped} unmapped`
                          : undefined
                      }
                      selected={tab === t.id}
                      onSelect={() => setTab(t.id)}
                    />
                  ))}
                </div>

                <div
                  role="tabpanel"
                  id="contributors-panel"
                  aria-labelledby={`contributors-tab-${tab}`}
                  aria-busy={data.stale}
                  className={
                    data.stale ? "opacity-60 transition-opacity" : undefined
                  }
                >
                  {tab === "stats" ? (
                    model.hasActivity ? (
                      <StatsTab model={model} onAskAgent={openAsk} />
                    ) : (
                      <div className="flex flex-col items-center gap-2 rounded-xl border border-stroke px-4 py-8 text-center">
                        <Users
                          className="size-5 text-content/35"
                          strokeWidth={1.75}
                        />
                        <p className="text-[13px] font-medium">
                          No commits in this period
                        </p>
                        <p className="text-[12px] text-content/50">
                          Try another period or branch.
                        </p>
                      </div>
                    )
                  ) : null}
                  {tab === "leaderboard" ? (
                    <LeaderboardTab
                      model={model}
                      onAskAgent={openAsk}
                      onChangeConfig={saveConfig}
                      editLocked={locked}
                      sidebarHost={sidebarHost}
                    />
                  ) : null}
                  {tab === "identities" ? (
                    <IdentitiesTab
                      model={model}
                      config={data.config}
                      onChangeConfig={saveConfig}
                      locked={locked}
                    />
                  ) : null}
                </div>
              </>
            ) : null}
          </div>
        </div>
        {ask && model && onStartAgentSession ? (
          <AskAgentDialog
            model={model}
            project={project}
            scope={ask.scope}
            providers={ask.providers}
            defaultProvider={ask.defaultProvider}
            onStart={onStartAgentSession}
            onClose={() => setAsk(null)}
          />
        ) : null}
        <div
          ref={setSidebarHost}
          className="pointer-events-none absolute inset-y-0 right-0 z-10 w-[400px] max-w-full"
        />
      </div>
    </div>
  );
}
