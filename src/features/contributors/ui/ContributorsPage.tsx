import { useEffect, useMemo, useRef, type KeyboardEvent } from "react";
import { OverlayNav } from "../../../app/shell/TitleBar";
import { WindowControls } from "../../../app/shell/WindowControls";
import { IS_MAC } from "../../../platform/tauri/platform";
import { prettyCwd, projectName } from "../../../shared/lib/paths";
import { ChartBreakoutSquare, Users } from "../../../shared/ui/icons";
import {
  isRemoteProjectPath,
  looksLikeProject,
  sameProjectPath,
} from "../../projects/model/recents";
import { Segmented, Select } from "../../settings/ui/primitives";
import { TicketContributorsView } from "../../ticket-contributors/ui/TicketContributorsView";
import type { AskAgentRequest } from "../model/askAgent";
import type { SnapshotScope } from "../model/snapshot";
import { ContributorsView } from "./ContributorsView";

export type ContributorsPageTab = "ticket" | "github";

const TABS: { id: ContributorsPageTab; label: string }[] = [
  { id: "ticket", label: "Ticket" },
  { id: "github", label: "GitHub" },
];

type Props = {
  tab: ContributorsPageTab;
  onTabChange: (tab: ContributorsPageTab) => void;
  /** The project the GitHub tab shows; null picks the default. */
  project: string | null;
  onProjectChange: (project: string) => void;
  /** The projects the app knows, in rail order. */
  projects: string[];
  /** The project the app is on now; the GitHub tab's default. */
  activeProject: string;
  /** Drops the page's own header: the new layout's title bar carries the chrome. */
  tabbed?: boolean;
  besideRail: boolean;
  compactRail: boolean;
  onClose: () => void;
  onToggleSidebar: () => void;
  onOpenSettings: () => void;
  onAskAgent?: (scope: SnapshotScope) => void;
  onStartAgentSession?: (
    project: string,
    request: AskAgentRequest,
  ) => Promise<void>;
};

/**
 * The project the GitHub tab shows: the chosen one, else the active project,
 * else the first local project; null when the app knows no project at all.
 */
function resolveContributorsProject(
  chosen: string | null,
  projects: string[],
  activeProject: string,
): string | null {
  if (chosen) return chosen;
  if (activeProject && looksLikeProject(activeProject)) return activeProject;
  return projects.find((path) => !isRemoteProjectPath(path)) ?? null;
}

/**
 * The Contributors page: the ClickUp ticket ranking and the git/GitHub
 * history of one project, as two tabs under one header.
 */
export function ContributorsPage({
  tab,
  onTabChange,
  project,
  onProjectChange,
  projects,
  activeProject,
  tabbed = false,
  besideRail,
  compactRail,
  onClose,
  onToggleSidebar,
  onOpenSettings,
  onAskAgent,
  onStartAgentSession,
}: Props) {
  const selected = resolveContributorsProject(project, projects, activeProject);
  // The shown project stays listed even when the rail no longer has it.
  const projectOptions = useMemo(() => {
    const paths =
      selected && !projects.some((path) => sameProjectPath(path, selected))
        ? [selected, ...projects]
        : projects;
    const names = paths.map(projectName);
    return paths.map((path, index) => ({
      value: path,
      // Two folders can share a name; the path tells them apart.
      label:
        names.filter((name) => name === names[index]).length > 1
          ? `${names[index]} (${prettyCwd(path)})`
          : names[index],
    }));
  }, [projects, selected]);
  const selectedValue =
    projectOptions.find((option) =>
      sameProjectPath(option.value, selected ?? ""),
    )?.value ?? "";

  const onTabKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const index = TABS.findIndex((t) => t.id === tab);
    let target: number;
    if (event.key === "ArrowRight") target = (index + 1) % TABS.length;
    else if (event.key === "ArrowLeft")
      target = (index - 1 + TABS.length) % TABS.length;
    else if (event.key === "Home") target = 0;
    else if (event.key === "End") target = TABS.length - 1;
    else return;
    event.preventDefault();
    const next = TABS[target];
    onTabChange(next.id);
    document.getElementById(`contributors-page-tab-${next.id}`)?.focus();
  };

  // Like the GitHub view: a plain Escape closes the page, on either tab. A
  // popover or text field gets the key first (it prevents it or is blurred),
  // and claiming it keeps App from stopping a running agent turn behind the
  // page.
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      const target = event.target;
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

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col text-content">
      {!tabbed ? (
        <div
          data-app-view-header
          className="flex h-10 shrink-0 select-none items-center border-b border-stroke"
          data-tauri-drag-region="deep"
        >
          {IS_MAC && compactRail ? <div className="w-4 shrink-0" /> : null}
          {IS_MAC && !besideRail ? <div className="w-[78px] shrink-0" /> : null}
          {besideRail ? null : (
            <OverlayNav onBack={onClose} onToggleSidebar={onToggleSidebar} />
          )}
          <div className="flex min-w-0 flex-1 items-center gap-2 px-3 text-[13px]">
            <ChartBreakoutSquare
              className="size-3.5 shrink-0 text-content/45"
              strokeWidth={1.75}
            />
            <span className="min-w-0 truncate text-content">Contributors</span>
          </div>
          {IS_MAC ? null : <WindowControls />}
        </div>
      ) : null}

      <div className="shrink-0 border-b border-stroke">
        <div className="mx-auto flex w-full max-w-5xl flex-wrap items-center gap-2.5 px-5 py-2.5 min-[560px]:px-8">
          <div onKeyDown={onTabKey}>
            <Segmented
              label="Contributors sources"
              value={tab}
              options={TABS.map((t) => ({ value: t.id, label: t.label }))}
              onChange={onTabChange}
              optionIdPrefix="contributors-page-tab"
              tabPanelId="contributors-page-panel"
            />
          </div>
          {tab === "github" && projectOptions.length > 0 ? (
            <div className="ml-auto">
              <Select
                label="Project"
                value={selectedValue}
                options={projectOptions}
                onChange={onProjectChange}
              />
            </div>
          ) : null}
        </div>
      </div>

      <div
        role="tabpanel"
        id="contributors-page-panel"
        aria-labelledby={`contributors-page-tab-${tab}`}
        className="flex min-h-0 min-w-0 flex-1 flex-col"
      >
        {tab === "ticket" ? (
          <TicketContributorsView
            tabbed
            besideRail={besideRail}
            compactRail={compactRail}
            onClose={onClose}
            onToggleSidebar={onToggleSidebar}
            onOpenSettings={onOpenSettings}
          />
        ) : selected ? (
          <ContributorsView
            key={selected}
            project={selected}
            tabbed
            besideRail={besideRail}
            compactRail={compactRail}
            onClose={onClose}
            onToggleSidebar={onToggleSidebar}
            onAskAgent={onAskAgent}
            onStartAgentSession={
              onStartAgentSession
                ? (request) => onStartAgentSession(selected, request)
                : undefined
            }
          />
        ) : (
          <div className="mx-auto flex w-full max-w-5xl flex-col px-5 py-6 min-[560px]:px-8">
            <div className="flex flex-col items-center gap-2 rounded-xl border border-stroke px-4 py-8 text-center">
              <Users className="size-5 text-content/35" strokeWidth={1.75} />
              <p className="text-[13px] font-medium">No projects yet</p>
              <p className="text-[12px] text-content/50">
                Open a project on this computer to see its contributors.
              </p>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
