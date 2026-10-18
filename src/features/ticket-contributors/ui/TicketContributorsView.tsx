import { Fragment, useEffect, useRef, useState } from "react";
import { OverlayNav } from "../../../app/shell/TitleBar";
import { WindowControls } from "../../../app/shell/WindowControls";
import { IS_MAC } from "../../../platform/tauri/platform";
import {
  ChevronLeft,
  ChevronRight,
  Loader,
  RefreshCw,
  X,
  ChartBreakoutSquare,
} from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { ScrollX } from "../../contributors/ui/ScrollX";
import { Segmented, Select } from "../../settings/ui/primitives";
import { useTicketBoard } from "../hooks/useTicketBoard";
import { excludedMembersOf } from "../model/board";
import type { MemberRow } from "../model/board";
import { CRITERIA, LEVELS, deliveredAt } from "../model/tickets";
import type {
  Criterion,
  Level,
  TicketRecord,
  WorkspaceScope,
} from "../model/tickets";
import { listWorkspaceSpaces } from "../model/ticketsApi";
import type { AiConfigStatus, TicketJobEvent } from "../model/ticketsApi";

export type TicketContributorsViewProps = {
  besideRail: boolean;
  compactRail: boolean;
  onClose(): void;
  onToggleSidebar(): void;
  onOpenSettings(): void;
};

type Model = ReturnType<typeof useTicketBoard>;
type Kind = "month" | "quarter" | "year" | "all";

const KINDS: { value: Kind; label: string }[] = [
  { value: "month", label: "Month" },
  { value: "quarter", label: "Quarter" },
  { value: "year", label: "Year" },
  { value: "all", label: "All" },
];

const PERIOD_ARROW =
  "flex size-7 items-center justify-center rounded-md text-content/60 hover:bg-content/8 hover:text-content disabled:pointer-events-none disabled:opacity-35";
const TH = "px-3 py-2 text-[12px] font-medium text-content/50";
const TD = "px-3 py-2 align-middle";
const CARD = "rounded-xl border border-stroke";
const CHECKBOX = "size-3.5 shrink-0 accent-[var(--color-accent,currentColor)]";
const ROW_ACTION =
  "shrink-0 rounded px-1.5 py-0.5 text-[12px] text-content/50 hover:bg-content/8 hover:text-content focus-visible:outline-2 focus-visible:outline-accent";
const AI_LEVEL = "";
const UNKNOWN_WORKSPACE = "";

const CRITERION_LABEL: Record<Criterion, string> = {
  ambiguity: "Ambiguity",
  investigation: "Investigation",
  scope: "Scope",
  risk: "Risk",
  coordination: "Coordination",
};

const RUNNER_LABEL: Record<AiConfigStatus["runner"], string> = {
  "claude-code": "Claude Code",
  api: "Anthropic API",
};

/** Thousands separators; fractions (a share of a ticket) show one decimal at most. */
const fmt = (n: number) =>
  n.toLocaleString("en-US", { maximumFractionDigits: 1 });

const plural = (n: number, one: string, many = `${one}s`) =>
  `${fmt(n)} ${n === 1 ? one : many}`;

const errorText = (error: unknown) =>
  error instanceof Error ? error.message : String(error);

const norm = (status: string) => status.trim().toLowerCase();

const isFinishedType = (type: string) => type === "done" || type === "closed";

type StatusInfo = { name: string; type: string };

/**
 * The statuses to list: the loaded ones, then any chosen name no loaded ticket
 * carries now, so a saved choice never disappears from the list.
 */
function listedStatuses(
  statuses: StatusInfo[],
  chosen: string[],
): StatusInfo[] {
  const listed = new Map(statuses.map((s) => [norm(s.name), s]));
  for (const name of [...chosen].sort((a, b) => a.localeCompare(b))) {
    if (!listed.has(norm(name))) {
      listed.set(norm(name), { name, type: "" });
    }
  }
  return [...listed.values()];
}

const live = (job: TicketJobEvent | null) =>
  job !== null && (job.state === "queued" || job.state === "running");

function CheckRow({
  label,
  hint,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  /** A small note after the label, such as a status type. */
  hint?: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label
      className={`flex items-center gap-2 text-[12px] ${
        disabled ? "text-content/40" : "text-content/80"
      }`}
    >
      <input
        type="checkbox"
        className={CHECKBOX}
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="min-w-0 truncate">{label}</span>
      {hint ? (
        <span className="shrink-0 text-[11px] text-content/40">{hint}</span>
      ) : null}
    </label>
  );
}

function ScopePanel({
  scope,
  statuses,
  tickets,
  onChange,
}: {
  scope: WorkspaceScope;
  statuses: StatusInfo[];
  /** The loaded tickets, where the excluded members' names come from. */
  tickets: TicketRecord[];
  onChange: (next: WorkspaceScope) => void;
}) {
  const [spaces, setSpaces] = useState<{ id: string; name: string }[] | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let stale = false;
    setSpaces(null);
    setError(null);
    listWorkspaceSpaces(scope.workspaceId)
      .then((list) => {
        if (!stale) setSpaces(list);
      })
      .catch((err: unknown) => {
        if (!stale) setError(errorText(err));
      });
    return () => {
      stale = true;
    };
  }, [scope.workspaceId]);

  // Never an empty list: no space picked means every space.
  const toggleSpace = (id: string, on: boolean) => {
    const current = scope.spaceIds ?? [];
    const next = on
      ? [...current.filter((s) => s !== id), id]
      : current.filter((s) => s !== id);
    onChange({ ...scope, spaceIds: next.length > 0 ? next : null });
  };
  const toggleStatus = (status: string, on: boolean) => {
    const rest = scope.excludedStatuses.filter((s) => norm(s) !== norm(status));
    onChange({ ...scope, excludedStatuses: on ? [...rest, status] : rest });
  };
  // Done and closed statuses are delivered unless excluded; any other status
  // is delivered only when chosen. Excluded always wins, so a status that is
  // excluded shows unchecked, and checking it lifts the exclusion.
  const has = (list: string[], status: string) =>
    list.some((s) => norm(s) === norm(status));
  const without = (list: string[], status: string) =>
    list.filter((s) => norm(s) !== norm(status));
  const deliveredChecked = (status: StatusInfo) =>
    !has(scope.excludedStatuses, status.name) &&
    (isFinishedType(status.type) || has(scope.deliveredStatuses, status.name));
  const toggleDelivered = (status: StatusInfo, on: boolean) => {
    if (isFinishedType(status.type)) {
      // Part of the default: leaving it out means excluding it.
      toggleStatus(status.name, !on);
      return;
    }
    const chosen = without(scope.deliveredStatuses, status.name);
    onChange({
      ...scope,
      deliveredStatuses: on ? [...chosen, status.name] : chosen,
      excludedStatuses: on
        ? without(scope.excludedStatuses, status.name)
        : scope.excludedStatuses,
    });
  };

  const excludedMembers = excludedMembersOf(tickets, scope);
  const excludedList = listedStatuses(statuses, scope.excludedStatuses);
  const deliveredList = listedStatuses(statuses, scope.deliveredStatuses);

  return (
    <div
      aria-label="Scope"
      role="group"
      className={`${CARD} mb-4 flex flex-col gap-3 px-4 py-3 @min-[560px]/ticket:flex-row @min-[560px]/ticket:gap-8`}
    >
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <span className="text-[12px] font-medium text-content/50">Spaces</span>
        <CheckRow
          label="All spaces"
          checked={scope.spaceIds === null}
          disabled={scope.spaceIds === null}
          onChange={() => onChange({ ...scope, spaceIds: null })}
        />
        {error ? (
          <p role="alert" className="text-[12px] text-red-400">
            {`Could not load the spaces: ${error}`}
          </p>
        ) : spaces === null ? (
          <span
            role="status"
            className="flex items-center gap-1.5 text-[12px] text-content/50"
          >
            <Loader className="size-3 animate-spin" strokeWidth={1.75} />
            Loading spaces...
          </span>
        ) : spaces.length === 0 ? (
          <p className="text-[12px] text-content/45">
            This token cannot list spaces. Tickets from every space are
            included.
          </p>
        ) : (
          spaces.map((space) => (
            <CheckRow
              key={space.id}
              label={space.name}
              checked={scope.spaceIds?.includes(space.id) ?? false}
              onChange={(on) => toggleSpace(space.id, on)}
            />
          ))
        )}
      </div>
      <div
        role="group"
        aria-label="Delivered statuses"
        className="flex min-w-0 flex-1 flex-col gap-1.5"
      >
        <span className="text-[12px] font-medium text-content/50">
          Delivered statuses
        </span>
        <p className="text-[12px] text-content/45">
          Tickets in these statuses count as delivered work.
        </p>
        {deliveredList.length === 0 ? (
          <p className="text-[12px] text-content/45">
            Statuses show up here once tickets are fetched.
          </p>
        ) : (
          deliveredList.map((status) => (
            <CheckRow
              key={norm(status.name)}
              label={status.name}
              hint={status.type}
              checked={deliveredChecked(status)}
              onChange={(on) => toggleDelivered(status, on)}
            />
          ))
        )}
      </div>
      <div
        role="group"
        aria-label="Excluded statuses"
        className="flex min-w-0 flex-1 flex-col gap-1.5"
      >
        <span className="text-[12px] font-medium text-content/50">
          Excluded statuses
        </span>
        {excludedList.length === 0 ? (
          <p className="text-[12px] text-content/45">
            Statuses show up here once tickets are fetched.
          </p>
        ) : (
          excludedList.map((status) => (
            <CheckRow
              key={norm(status.name)}
              label={status.name}
              checked={scope.excludedStatuses.some(
                (s) => norm(s) === norm(status.name),
              )}
              onChange={(on) => toggleStatus(status.name, on)}
            />
          ))
        )}
      </div>
      <div
        role="group"
        aria-label="Excluded members"
        className="flex min-w-0 flex-1 flex-col gap-1.5"
      >
        <span className="text-[12px] font-medium text-content/50">
          Excluded members
        </span>
        {excludedMembers.length === 0 ? (
          <p className="text-[12px] text-content/45">
            Nobody is excluded. Use Exclude on a member of the board.
          </p>
        ) : (
          excludedMembers.map((member) => (
            <div
              key={member.id}
              className="flex items-center gap-2 text-[12px] text-content/80"
            >
              <span className="min-w-0 flex-1 truncate">{member.name}</span>
              <button
                type="button"
                aria-label={`Restore ${member.name} to the board`}
                onClick={() =>
                  onChange({
                    ...scope,
                    excludedMembers: scope.excludedMembers.filter(
                      (m) => m.id !== member.id,
                    ),
                  })
                }
                className={ROW_ACTION}
              >
                Restore
              </button>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function Problem({
  message,
  onOpenSettings,
  onRetry,
}: {
  message: string;
  onOpenSettings: () => void;
  onRetry?: () => void;
}) {
  return (
    <div className="flex flex-col items-center gap-3 rounded-xl border border-stroke px-4 py-8 text-center">
      <p className="text-[13px]">{message}</p>
      <div className="flex gap-2">
        <SecondaryButton onClick={onOpenSettings}>
          Open Settings
        </SecondaryButton>
        {onRetry ? (
          <SecondaryButton onClick={onRetry}>Retry</SecondaryButton>
        ) : null}
      </div>
    </div>
  );
}

function JobProgress({
  job,
  onCancel,
}: {
  job: TicketJobEvent;
  onCancel: () => void;
}) {
  return (
    <div className="flex items-center gap-3 border-t border-stroke px-4 py-3 text-[12px]">
      <span className="shrink-0 text-content/60">
        {job.kind === "fetch"
          ? "Fetching tickets from ClickUp..."
          : "Scoring tickets..."}
      </span>
      <div
        role="progressbar"
        aria-label="Ticket job progress"
        aria-valuemin={0}
        aria-valuemax={job.total}
        aria-valuenow={job.done}
        className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-content/10"
      >
        <div
          className="h-full bg-accent"
          style={{
            width: `${job.total > 0 ? Math.min(100, (job.done / job.total) * 100) : 0}%`,
          }}
        />
      </div>
      <span className="shrink-0 tabular-nums text-content/60">
        {`${fmt(job.done)}/${fmt(job.total)}`}
        {job.failed > 0 ? `, ${fmt(job.failed)} failed` : ""}
      </span>
      <SecondaryButton onClick={onCancel}>Cancel</SecondaryButton>
    </div>
  );
}

function ScoreCard({ m, periodPhrase }: { m: Model; periodPhrase: string }) {
  const { board, toAssess, estimate, ai, job, settings } = m;
  const busy = live(job);
  // Typed text stays as typed until blur or Enter; an empty field snaps back.
  const [draft, setDraft] = useState<string | null>(null);
  const commitDraft = () => {
    if (draft === null) return;
    setDraft(null);
    const typed = Number(draft);
    if (draft.trim() === "" || !Number.isFinite(typed)) return;
    m.setPercent(Math.min(100, Math.max(0, Math.round(typed))));
  };
  const unscored = board.total - board.assessed;
  const excluded = settings.scope?.excludedStatuses ?? [];
  const allExcluded =
    m.statuses.length > 0 &&
    m.statuses.every((s) => excluded.some((e) => norm(e) === norm(s.name)));
  const failed =
    job !== null && job.kind === "assess" && !busy && job.failed > 0
      ? job.failed
      : 0;

  return (
    <section aria-label="Score tickets" className={`${CARD} mb-4`}>
      <div className="flex flex-col gap-3 px-4 py-3 text-[13px]">
        <p>
          {`${plural(board.total, "delivered ticket")} ${periodPhrase}. ${fmt(board.assessed)} scored, ${fmt(unscored)} not scored yet.`}
        </p>
        {allExcluded ? (
          <p className="text-[12px] text-amber-700 dark:text-amber-400">
            Every status is excluded, so no ticket counts.
          </p>
        ) : null}
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <span className="text-[12px] text-content/60">Score coverage</span>
          <input
            type="range"
            min={0}
            max={100}
            step={1}
            aria-label="Score coverage slider"
            value={settings.percent}
            onChange={(event) => m.setPercent(Number(event.target.value))}
            className="sidebar-opacity-slider w-44 max-w-full"
          />
          <span className="flex items-center gap-1 text-[12px]">
            <input
              type="number"
              min={0}
              max={100}
              step={1}
              aria-label="Score coverage"
              value={draft ?? String(settings.percent)}
              onChange={(event) => setDraft(event.target.value)}
              onBlur={commitDraft}
              onKeyDown={(event) => {
                if (event.key === "Enter") commitDraft();
              }}
              className="w-16 rounded-md border border-content/10 bg-content/5 px-2 py-1 text-right tabular-nums outline-none hover:border-content/20 focus:border-accent"
            />
            %
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <p className="mr-auto text-[12px] text-content/60">
            {toAssess === 0
              ? "Coverage already reached. Nothing to score."
              : `Scoring ${fmt(toAssess)} more ${toAssess === 1 ? "ticket" : "tickets"} with ${
                  ai
                    ? `${ai.model} (${RUNNER_LABEL[ai.runner]})`
                    : "the configured model"
                }.${
                  estimate
                    ? ` About ${fmt(Math.round(estimate.inputTokens))} input tokens (estimate).`
                    : ""
                }`}
          </p>
          <SecondaryButton
            disabled={toAssess === 0 || busy}
            onClick={() => void m.score()}
          >
            Score
          </SecondaryButton>
        </div>
      </div>
      {job !== null && busy ? (
        <JobProgress job={job} onCancel={() => void m.cancel()} />
      ) : null}
      {failed > 0 ? (
        <p className="border-t border-stroke px-4 py-2.5 text-[12px] text-amber-700 dark:text-amber-400">
          {`${plural(failed, "ticket")} could not be scored and ${failed === 1 ? "stays" : "stay"} at 0 points.`}
        </p>
      ) : null}
      {board.datesApproximate ? (
        <p className="border-t border-stroke px-4 py-2.5 text-[12px] text-amber-700 dark:text-amber-400">
          Some delivery dates are approximate (last update). Turn on the Total
          time in Status ClickApp in ClickUp for exact dates.
        </p>
      ) : null}
    </section>
  );
}

type Entry = MemberRow["ticketList"][number];

function TicketRow({
  entry,
  selected,
  onSelect,
  onOverride,
}: {
  entry: Entry;
  selected: boolean;
  onSelect: (on: boolean) => void;
  onOverride: (level: Level | null, note: string | null) => void;
}) {
  const { ticket, level, share, points } = entry;
  const edited = ticket.overrideLevel !== null;
  const [note, setNote] = useState(ticket.overrideNote ?? "");
  const [showReasons, setShowReasons] = useState(false);
  useEffect(() => {
    setNote(ticket.overrideNote ?? "");
  }, [ticket.overrideNote]);

  const commitNote = () => {
    const next = note.trim() || null;
    if (next === (ticket.overrideNote ?? null)) return;
    if (ticket.overrideLevel) onOverride(ticket.overrideLevel, next);
  };

  return (
    <>
      <tr
        data-ticket={ticket.id}
        data-edited={edited ? "true" : undefined}
        className="border-t border-stroke hover:bg-content/3"
      >
        <td className={TD}>
          <input
            type="checkbox"
            aria-label={`Select ${ticket.title}`}
            className={CHECKBOX}
            checked={selected}
            onChange={(event) => onSelect(event.target.checked)}
          />
        </td>
        <td className={`${TD} max-w-[22rem]`}>
          <a
            href={ticket.url}
            target="_blank"
            rel="noreferrer"
            className="block truncate font-medium text-accent hover:underline"
            title={ticket.title}
          >
            {ticket.title}
          </a>
          <span className="text-[11px] text-content/45">
            {ticket.customId ?? `#${ticket.id}`}
          </span>
        </td>
        <td className={`${TD} whitespace-nowrap`}>
          {level ? (
            <span className="inline-flex items-center gap-1.5">
              <span className="rounded-full border border-stroke px-1.5 text-[11px] font-medium">
                {level}
              </span>
              {edited ? (
                <span className="text-[11px] text-content/50">edited</span>
              ) : null}
            </span>
          ) : (
            <span className="text-[11px] text-content/45">Not scored</span>
          )}
        </td>
        <td className={`${TD} whitespace-nowrap text-right tabular-nums`}>
          {level ? fmt(share) : "-"}
          {level && share !== points ? (
            <span className="ml-1 text-[11px] text-content/45">
              {`of ${fmt(points)}`}
            </span>
          ) : null}
        </td>
        {CRITERIA.map((criterion) => (
          <td
            key={criterion}
            className={`${TD} text-right tabular-nums`}
            data-criterion={criterion}
          >
            {ticket.assessment ? (
              <button
                type="button"
                aria-expanded={showReasons}
                aria-label={`${CRITERION_LABEL[criterion]} ${ticket.assessment.criteria[criterion]}, ${showReasons ? "hide" : "show"} reasons`}
                title={ticket.assessment.reasons[criterion]}
                onClick={() => setShowReasons((open) => !open)}
                className="rounded underline decoration-dotted decoration-content/30 underline-offset-2 hover:text-accent focus-visible:outline-2 focus-visible:outline-accent"
              >
                {ticket.assessment.criteria[criterion]}
              </button>
            ) : (
              <span className="text-content/30">-</span>
            )}
          </td>
        ))}
        <td className={TD}>
          <Select
            label={`Level of ${ticket.title}`}
            value={ticket.overrideLevel ?? AI_LEVEL}
            options={[
              { value: AI_LEVEL, label: "AI level" },
              ...LEVELS.map((l) => ({ value: l, label: l })),
            ]}
            onChange={(value) =>
              onOverride(
                value === AI_LEVEL ? null : (value as Level),
                value === AI_LEVEL ? null : note.trim() || null,
              )
            }
          />
        </td>
        <td className={TD}>
          <input
            type="text"
            aria-label={`Note for ${ticket.title}`}
            placeholder={
              edited ? "Why this level" : "Set a level to add a note"
            }
            disabled={!edited}
            value={note}
            onChange={(event) => setNote(event.target.value)}
            onBlur={commitNote}
            onKeyDown={(event) => {
              if (event.key === "Enter") event.currentTarget.blur();
            }}
            className="w-40 rounded-md border border-content/10 bg-content/5 px-2 py-1 text-[12px] outline-none placeholder:text-content/35 hover:border-content/20 focus:border-accent disabled:opacity-50"
          />
        </td>
      </tr>
      {showReasons && ticket.assessment ? (
        <tr data-reasons={ticket.id} className="bg-content/3">
          <td />
          <td colSpan={10} className="px-3 pb-2.5 pt-1">
            <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-[12px]">
              {CRITERIA.map((criterion) => (
                <Fragment key={criterion}>
                  <dt className="text-content/50">
                    {CRITERION_LABEL[criterion]}
                  </dt>
                  <dd>
                    {ticket.assessment!.reasons[criterion] ||
                      "No reason given."}
                  </dd>
                </Fragment>
              ))}
            </dl>
          </td>
        </tr>
      ) : null}
    </>
  );
}

function MemberTickets({
  row,
  busy,
  override,
  reassess,
}: {
  row: MemberRow;
  busy: boolean;
  override: Model["override"];
  reassess: Model["reassess"];
}) {
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const entries = [...row.ticketList].sort((a, b) =>
    deliveredAt(b.ticket).at.localeCompare(deliveredAt(a.ticket).at),
  );
  const chosen = entries.filter((e) => selected.has(e.ticket.id));
  const select = (id: string, on: boolean) =>
    setSelected((current) => {
      const next = new Set(current);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  if (entries.length === 0) {
    return (
      <p className="border-t border-stroke bg-content/[0.02] px-3 py-3 text-[12px] text-content/50">
        No delivered tickets in this period.
      </p>
    );
  }

  return (
    <div className="border-t border-stroke bg-content/[0.02]">
      <div className="flex items-center justify-end gap-2 px-3 py-2">
        <SecondaryButton
          disabled={chosen.length === 0 || busy}
          onClick={() => {
            void reassess(chosen.map((e) => e.ticket.id));
            setSelected(new Set());
          }}
        >
          Re-assess selected
        </SecondaryButton>
      </div>
      <ScrollX label={`Tickets of ${row.name}`}>
        <table className="w-full border-collapse text-left text-[13px]">
          <thead>
            <tr>
              <th scope="col" className={`${TH} w-8`}>
                <span className="sr-only">Select</span>
              </th>
              <th scope="col" className={TH}>
                Ticket
              </th>
              <th scope="col" className={TH}>
                Level
              </th>
              <th scope="col" className={`${TH} text-right`}>
                Points
              </th>
              {CRITERIA.map((criterion) => (
                <th key={criterion} scope="col" className={`${TH} text-right`}>
                  {CRITERION_LABEL[criterion]}
                </th>
              ))}
              <th scope="col" className={TH}>
                Set level
              </th>
              <th scope="col" className={TH}>
                Note
              </th>
            </tr>
          </thead>
          <tbody>
            {entries.map((entry) => (
              <TicketRow
                key={entry.ticket.id}
                entry={entry}
                selected={selected.has(entry.ticket.id)}
                onSelect={(on) => select(entry.ticket.id, on)}
                onOverride={(level, note) =>
                  void override(entry.ticket.id, level, note)
                }
              />
            ))}
          </tbody>
        </table>
      </ScrollX>
    </div>
  );
}

function Leaderboard({ m }: { m: Model }) {
  const { board } = m;
  const [open, setOpen] = useState<string | null>(null);
  const busy = live(m.job);
  const { unassigned } = board;
  const scope = m.settings.scope;
  // Applies at once: the board is rebuilt from the loaded tickets, no refetch.
  const excludeMember = (member: { id: string; name: string }) => {
    if (!scope || scope.excludedMembers.some((e) => e.id === member.id)) return;
    // The row leaves the board; it must not come back expanded after a restore.
    if (open === member.id) setOpen(null);
    m.setScope({
      ...scope,
      excludedMembers: [
        ...scope.excludedMembers,
        { id: member.id, name: member.name },
      ],
    });
  };

  if (board.members.length === 0 && unassigned.tickets === 0) {
    return (
      <p
        className={`${CARD} px-4 py-8 text-center text-[12px] text-content/50`}
      >
        No delivered tickets in this period.
      </p>
    );
  }

  return (
    <section aria-label="Leaderboard" className={CARD}>
      <ScrollX label="Leaderboard table">
        <table className="w-full border-collapse text-left text-[13px]">
          <thead>
            <tr>
              <th scope="col" className={`${TH} w-10`}>
                Rank
              </th>
              <th scope="col" className={TH}>
                Member
              </th>
              <th scope="col" className={`${TH} text-right`}>
                Points
              </th>
              <th scope="col" className={`${TH} text-right`}>
                Tickets
              </th>
              <th scope="col" className={`${TH} text-right`}>
                Coverage
              </th>
              <th
                scope="col"
                title="In a working status during this period. Not scored."
                className={`${TH} text-right`}
              >
                Active
              </th>
              {LEVELS.map((level) => (
                <th key={level} scope="col" className={`${TH} text-right`}>
                  {level}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {board.members.map((row, index) => {
              const expanded = open === row.id;
              return (
                <Fragment key={row.id}>
                  <tr className="border-t border-stroke hover:bg-content/3">
                    <td className={`${TD} tabular-nums text-content/60`}>
                      {index + 1}
                    </td>
                    <td className={TD}>
                      <div className="flex items-center gap-2">
                        <button
                          type="button"
                          aria-expanded={expanded}
                          onClick={() => setOpen(expanded ? null : row.id)}
                          className="flex items-center gap-1.5 rounded font-medium hover:text-accent focus-visible:outline-2 focus-visible:outline-accent"
                        >
                          <ChevronRight
                            className={`size-3 shrink-0 text-content/45 transition-transform ${expanded ? "rotate-90" : ""}`}
                            strokeWidth={1.75}
                          />
                          {row.name}
                        </button>
                        <button
                          type="button"
                          aria-label={`Exclude ${row.name} from the board`}
                          title="Exclude from the board"
                          onClick={() => excludeMember(row)}
                          className={ROW_ACTION}
                        >
                          <X className="size-3" strokeWidth={1.75} />
                        </button>
                      </div>
                    </td>
                    <td className={`${TD} text-right tabular-nums`}>
                      {fmt(row.points)}
                    </td>
                    <td className={`${TD} text-right tabular-nums`}>
                      {fmt(row.tickets)}
                    </td>
                    <td className={`${TD} text-right tabular-nums`}>
                      {`${fmt(row.assessed)}/${fmt(row.tickets)}`}
                    </td>
                    <td
                      className={`${TD} text-right tabular-nums ${row.active === 0 ? "text-content/30" : ""}`}
                    >
                      {fmt(row.active)}
                    </td>
                    {LEVELS.map((level) => (
                      <td
                        key={level}
                        className={`${TD} text-right tabular-nums ${row.levels[level] === 0 ? "text-content/30" : ""}`}
                      >
                        {fmt(row.levels[level])}
                      </td>
                    ))}
                  </tr>
                  {expanded ? (
                    <tr data-member-tickets={row.id}>
                      <td colSpan={6 + LEVELS.length} className="p-0">
                        <MemberTickets
                          row={row}
                          busy={busy}
                          override={m.override}
                          reassess={m.reassess}
                        />
                      </td>
                    </tr>
                  ) : null}
                </Fragment>
              );
            })}
            {unassigned.tickets > 0 ? (
              <tr className="border-t border-stroke text-content/60">
                <td className={`${TD} tabular-nums`}>-</td>
                <td className={TD}>Unassigned</td>
                <td className={`${TD} text-right tabular-nums`}>-</td>
                <td className={`${TD} text-right tabular-nums`}>
                  {fmt(unassigned.tickets)}
                </td>
                <td className={`${TD} text-right tabular-nums`}>
                  {`${fmt(unassigned.assessed)}/${fmt(unassigned.tickets)}`}
                </td>
                <td className={`${TD} text-right tabular-nums`}>-</td>
                <td colSpan={LEVELS.length} />
              </tr>
            ) : null}
          </tbody>
        </table>
      </ScrollX>
    </section>
  );
}

/**
 * The app-level Ticket Contributors view: ClickUp users ranked by the points
 * of the delivered tickets the AI assessed. All counts come from the board; the
 * loaded ticket window is wider than the period and is never counted here.
 */
export function TicketContributorsView({
  besideRail,
  compactRail,
  onClose,
  onToggleSidebar,
  onOpenSettings,
}: TicketContributorsViewProps) {
  const m = useTicketBoard();
  const { connected, settings, period, workspaces } = m;
  const scope = settings.scope;
  const [scopeOpen, setScopeOpen] = useState(false);
  // The token's list is known and the saved workspace is not in it.
  const unreachable =
    scope !== null &&
    m.workspacesState === "ready" &&
    !workspaces.some((w) => w.id === scope.workspaceId);
  const fetching = m.job?.kind === "fetch" && live(m.job);
  const [exporting, setExporting] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);
  const [exported, setExported] = useState(false);
  // Bumped when the workspace or period changes: an export that started
  // before then is for another report, so its outcome is dropped.
  const exportRun = useRef(0);
  useEffect(() => {
    exportRun.current += 1;
    setExported(false);
    setExportError(null);
  }, [scope?.workspaceId, period.id]);
  const exportReport = async () => {
    const run = (exportRun.current += 1);
    setExporting(true);
    setExported(false);
    setExportError(null);
    try {
      const path = await m.exportReport();
      if (run === exportRun.current) setExported(path !== null);
    } catch (error) {
      if (run === exportRun.current) {
        setExportError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      setExporting(false);
    }
  };

  const kind: Kind = period.kind === "7d" ? "month" : period.kind;
  const periodPhrase =
    period.kind === "all" ? "across all time" : `in ${period.label}`;

  // A saved workspace the token no longer lists stays selectable as the current value.
  const workspaceOptions = [
    ...(scope ? [] : [{ value: UNKNOWN_WORKSPACE, label: "Pick a workspace" }]),
    ...workspaces.map((w) => ({ value: w.id, label: w.name })),
    ...(scope && !workspaces.some((w) => w.id === scope.workspaceId)
      ? [{ value: scope.workspaceId, label: scope.name || scope.workspaceId }]
      : []),
  ];

  const pickWorkspace = (id: string) => {
    if (id === UNKNOWN_WORKSPACE || id === scope?.workspaceId) return;
    const workspace = workspaces.find((w) => w.id === id);
    if (!workspace) return;
    m.setScope({
      workspaceId: workspace.id,
      name: workspace.name,
      spaceIds: null,
      excludedStatuses: [],
      deliveredStatuses: [],
      excludedMembers: [],
    });
  };

  return (
    <div
      role="region"
      aria-label="Ticket Contributors"
      data-app-ticket-contributors
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
          <ChartBreakoutSquare
            className="size-3.5 shrink-0 text-content/45"
            strokeWidth={1.75}
          />
          <span className="min-w-0 truncate text-content">
            Ticket Contributors
          </span>
        </div>
        {IS_MAC ? null : <WindowControls />}
      </div>

      <div className="@container/ticket min-h-0 flex-1 overflow-y-auto overscroll-none">
        <div className="mx-auto w-full max-w-5xl px-5 py-6 pb-16 @min-[560px]/ticket:px-8">
          <div className="mb-4 flex flex-wrap items-center gap-2.5">
            <h1 className="mr-auto text-[20px] font-semibold leading-tight">
              Ticket Contributors
            </h1>
            {connected === true && m.workspacesState !== "error" ? (
              <>
                <Select
                  label="Workspace"
                  value={scope?.workspaceId ?? UNKNOWN_WORKSPACE}
                  options={workspaceOptions}
                  onChange={pickWorkspace}
                />
                <Segmented<Kind>
                  label="Period"
                  value={kind}
                  options={KINDS}
                  onChange={m.setKind}
                />
                {kind !== "all" ? (
                  <div
                    role="group"
                    aria-label="Period navigation"
                    className="flex items-center gap-0.5 text-[12px]"
                  >
                    <button
                      type="button"
                      aria-label="Previous period"
                      onClick={m.goPrevious}
                      className={PERIOD_ARROW}
                    >
                      <ChevronLeft className="size-3.5" strokeWidth={1.75} />
                    </button>
                    <span
                      aria-live="polite"
                      className="min-w-[4.5rem] text-center tabular-nums"
                    >
                      {period.label}
                    </span>
                    <button
                      type="button"
                      aria-label="Next period"
                      disabled={!m.hasNext}
                      onClick={m.goNext}
                      className={PERIOD_ARROW}
                    >
                      <ChevronRight className="size-3.5" strokeWidth={1.75} />
                    </button>
                  </div>
                ) : null}
                <button
                  type="button"
                  aria-label="Refresh from ClickUp"
                  title="Refresh from ClickUp"
                  disabled={scope === null || unreachable || fetching}
                  onClick={() => void m.refresh()}
                  className="rounded-md p-1.5 text-content/40 hover:bg-content/8 hover:text-content disabled:opacity-30"
                >
                  {fetching ? (
                    <Loader
                      className="size-3.5 animate-spin"
                      strokeWidth={1.75}
                    />
                  ) : (
                    <RefreshCw className="size-3.5" strokeWidth={1.75} />
                  )}
                </button>
                <SecondaryButton
                  disabled={
                    scope === null ||
                    unreachable ||
                    fetching ||
                    m.pendingFetches > 0 ||
                    exporting ||
                    m.board.total === 0
                  }
                  onClick={() => void exportReport()}
                >
                  {exporting ? "Preparing report..." : "Export report"}
                </SecondaryButton>
                {scope && !unreachable ? (
                  <SecondaryButton
                    aria-expanded={scopeOpen}
                    onClick={() => setScopeOpen((open) => !open)}
                  >
                    Scope
                    <ChevronRight
                      className={`size-3 transition-transform ${scopeOpen ? "rotate-90" : ""}`}
                      strokeWidth={1.75}
                    />
                  </SecondaryButton>
                ) : null}
              </>
            ) : null}
          </div>

          {exported ? (
            <p
              role="status"
              className="mb-4 rounded-xl border border-stroke px-4 py-2.5 text-[12px] text-content/70"
            >
              Report saved.
            </p>
          ) : null}

          {m.error ? (
            <p
              role="alert"
              className="mb-4 rounded-xl border border-stroke px-4 py-2.5 text-[12px] text-red-400"
            >
              {m.error}
            </p>
          ) : null}

          {exportError ? (
            <p
              role="alert"
              className="mb-4 rounded-xl border border-stroke px-4 py-2.5 text-[12px] text-red-400"
            >
              {exportError}
            </p>
          ) : null}

          {connected !== true && m.connectError !== null ? (
            <Problem
              message={`Could not reach ClickUp. ${m.connectError}`}
              onOpenSettings={onOpenSettings}
              onRetry={m.retry}
            />
          ) : connected === null ? (
            <span
              role="status"
              className="flex items-center gap-2 text-[12px] text-content/50"
            >
              <Loader className="size-3.5 animate-spin" strokeWidth={1.75} />
              Checking ClickUp...
            </span>
          ) : connected === false ? (
            <Problem
              message="Connect ClickUp in Settings to score tickets."
              onOpenSettings={onOpenSettings}
            />
          ) : m.workspacesState === "error" ? (
            <Problem
              message={`Could not load ClickUp workspaces. ${m.workspacesError ?? ""}`.trim()}
              onOpenSettings={onOpenSettings}
              onRetry={m.retry}
            />
          ) : scope === null ? (
            m.workspacesState === "loading" ? (
              <span
                role="status"
                className="flex items-center gap-2 text-[12px] text-content/50"
              >
                <Loader className="size-3.5 animate-spin" strokeWidth={1.75} />
                Loading workspaces...
              </span>
            ) : workspaces.length === 0 ? (
              <Problem
                message="This ClickUp token has no workspaces."
                onOpenSettings={onOpenSettings}
              />
            ) : (
              <p className="rounded-xl border border-stroke px-4 py-8 text-center text-[13px]">
                Pick a ClickUp workspace.
              </p>
            )
          ) : unreachable ? (
            <Problem
              message={`This ClickUp token cannot access ${scope.name || scope.workspaceId}. Pick another workspace or update the token in Settings.`}
              onOpenSettings={onOpenSettings}
            />
          ) : (
            <>
              {scopeOpen ? (
                <ScopePanel
                  scope={scope}
                  statuses={m.statuses}
                  tickets={m.tickets}
                  onChange={m.setScope}
                />
              ) : null}
              <ScoreCard m={m} periodPhrase={periodPhrase} />
              <Leaderboard m={m} />
            </>
          )}
        </div>
      </div>
    </div>
  );
}
