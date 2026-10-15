import { Fragment, useState, type ReactNode } from "react";
import { axisTicks } from "../model/axis";
import { formatCompact } from "../model/compact";
import { formatDelta, relativeTime } from "../model/format";
import type { ContributorsModel } from "../model/pipeline";
import type { SnapshotScope } from "../model/snapshot";
import type { Stats, StatsRow } from "../model/stats";
import { Avatar } from "./Avatar";
import { ScrollX } from "./ScrollX";

type Props = {
  model: ContributorsModel;
  onAskAgent?: (scope: SnapshotScope) => void;
};

const FALLBACK_COLOR = "#8a8f98";
const CHART_W = 1000;
const CHART_H = 180;
const CHART_PAD = 8;

/** A signed change line: arrow, text and color together so color is never the only cue. */
function Change({
  value,
  text,
  suffix,
}: {
  value: number | null;
  text: string;
  suffix?: string;
}) {
  if (value === null) return <div className="mt-0.5 h-4" />;
  const tone =
    value > 0
      ? "text-diff-add-fg"
      : value < 0
        ? "text-diff-del-fg"
        : "text-content/45";
  const arrow = value > 0 ? "▲ " : value < 0 ? "▼ " : "";
  return (
    <div className={`mt-0.5 h-4 text-[11px] ${tone}`}>
      {arrow}
      {text}
      {suffix ? <span className="text-content/45"> {suffix}</span> : null}
    </div>
  );
}

function Kpi({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="rounded-xl border border-stroke bg-content/3 px-3.5 py-3">
      <div className="text-[12px] text-content/50">{label}</div>
      {children}
    </div>
  );
}

export function KpiCards({ model }: { model: ContributorsModel }) {
  const { kpis } = model.stats;
  const versus =
    model.period.kind === "7d"
      ? "vs prev 7d"
      : `vs ${model.previousPeriod.label}`;
  const commitsDelta = kpis.commits.delta;
  const linesDelta = kpis.lines.delta;
  const prsDelta = kpis.prsMerged.delta;
  return (
    <div className="grid grid-cols-2 gap-2.5 @min-[720px]/contrib:grid-cols-4">
      <Kpi label="Commits">
        <div className="mt-0.5 text-[20px] font-semibold tabular-nums">
          {kpis.commits.value}
        </div>
        <Change
          value={commitsDelta}
          text={commitsDelta === null ? "" : formatDelta(commitsDelta)}
          suffix={versus}
        />
      </Kpi>
      <Kpi label="Contributors">
        <div className="mt-0.5 text-[20px] font-semibold tabular-nums">
          {kpis.contributors.value}{" "}
          <span className="text-[12px] font-normal text-content/50">
            from {kpis.contributors.identities}{" "}
            {kpis.contributors.identities === 1 ? "identity" : "identities"}
          </span>
        </div>
        <Change
          value={kpis.contributors.newcomers > 0 ? 1 : null}
          text={`${kpis.contributors.newcomers} new`}
        />
      </Kpi>
      <Kpi label="Lines changed">
        <div className="mt-0.5 text-[20px] font-semibold tabular-nums">
          <span className="text-diff-add-fg">
            +{formatCompact(kpis.lines.added)}
          </span>{" "}
          <span className="text-diff-del-fg">
            -{formatCompact(kpis.lines.deleted)}
          </span>
        </div>
        <Change
          value={linesDelta}
          text={linesDelta === null ? "" : formatDelta(linesDelta)}
          suffix={versus}
        />
      </Kpi>
      <Kpi label="PRs merged">
        <div className="mt-0.5 text-[20px] font-semibold tabular-nums">
          {kpis.prsMerged.value}
        </div>
        <Change
          value={prsDelta}
          text={
            prsDelta === null
              ? ""
              : prsDelta > 0
                ? `+${prsDelta}`
                : String(prsDelta)
          }
          suffix={versus}
        />
      </Kpi>
    </div>
  );
}

function chartSummary(stats: Stats): string {
  const unit = stats.granularity;
  const { buckets } = stats;
  const totals = buckets.map((b) => ({
    label: b.label,
    total: Object.values(b.byPerson).reduce((a, c) => a + c, 0),
  }));
  const all = totals.reduce((sum, b) => sum + b.total, 0);
  const busiest = totals.reduce(
    (best, b) => (b.total > best.total ? b : best),
    totals[0],
  );
  if (!busiest || all === 0) {
    return `Commit activity per ${unit}: no commits in ${buckets.length} ${unit}s.`;
  }
  return `Commit activity per ${unit}: ${all} commits over ${buckets.length} ${unit}s, busiest ${unit} ${busiest.label} with ${busiest.total}.`;
}

export function CommitChart({ stats }: { stats: Stats }) {
  const { buckets, legend } = stats;
  const colors = new Map(legend.map((l) => [l.personId, l.color]));
  const names = new Map(legend.map((l) => [l.personId, l.name]));
  const order = legend.map((l) => l.personId);
  const totals = buckets.map((b) =>
    Object.values(b.byPerson).reduce((a, c) => a + c, 0),
  );
  const peak = Math.max(0, ...totals);
  const max = Math.max(1, peak);
  const slot = buckets.length > 0 ? CHART_W / buckets.length : CHART_W;
  const barW = slot * 0.7;
  const usable = CHART_H - CHART_PAD * 2;
  const ticks = axisTicks(buckets.length);
  const last = buckets.length - 1;

  return (
    <div className="px-3 py-2">
      <div className="relative">
        <svg
          role="img"
          aria-label={chartSummary(stats)}
          viewBox={`0 0 ${CHART_W} ${CHART_H}`}
          preserveAspectRatio="none"
          className="block h-[180px] w-full"
        >
          <line
            data-baseline
            x1={0}
            x2={CHART_W}
            y1={CHART_H - CHART_PAD}
            y2={CHART_H - CHART_PAD}
            stroke="currentColor"
            strokeOpacity={0.15}
            vectorEffect="non-scaling-stroke"
          />
          {buckets.map((bucket, i) => {
            let y = CHART_H - CHART_PAD;
            const x = i * slot + (slot - barW) / 2;
            const lines = order
              .filter((id) => (bucket.byPerson[id] ?? 0) > 0)
              .map((id) => `${names.get(id) ?? id}: ${bucket.byPerson[id]}`);
            return (
              <g key={bucket.start} data-bucket={bucket.start}>
                <title>
                  {[
                    `${bucket.label}: ${totals[i]} ${totals[i] === 1 ? "commit" : "commits"}`,
                    ...lines,
                  ].join("\n")}
                </title>
                {order
                  .filter((id) => (bucket.byPerson[id] ?? 0) > 0)
                  .map((id) => {
                    const count = bucket.byPerson[id];
                    const h = (count / max) * usable;
                    y -= h;
                    return (
                      <rect
                        key={id}
                        data-person={id}
                        x={x}
                        y={y}
                        width={barW}
                        height={h}
                        rx={1}
                        fill={colors.get(id) ?? FALLBACK_COLOR}
                      >
                        <title>{`${names.get(id) ?? id}: ${count}`}</title>
                      </rect>
                    );
                  })}
              </g>
            );
          })}
        </svg>
        <span
          data-ymax
          aria-hidden
          className="pointer-events-none absolute left-0 top-0 text-[10px] tabular-nums text-content/40"
        >
          {peak}
        </span>
      </div>
      <div
        aria-hidden
        className="relative mt-1 h-4 text-[10px] text-content/45"
      >
        {ticks.map((i) => (
          <span
            key={buckets[i].start}
            data-axis
            className="absolute whitespace-nowrap"
            style={
              i === 0
                ? { left: 0 }
                : i === last
                  ? { right: 0 }
                  : {
                      left: `${((i + 0.5) / buckets.length) * 100}%`,
                      transform: "translateX(-50%)",
                    }
            }
          >
            {buckets[i].short}
          </span>
        ))}
      </div>
    </div>
  );
}

const LEGEND_LIMIT = 8;

/** The busiest contributors; the rest fold into one chip so a big team does not push the chart down. */
function Legend({ stats }: { stats: Stats }) {
  const commits = new Map(stats.rows.map((r) => [r.personId, r.commits]));
  const ranked = stats.legend
    .map((entry, index) => ({ entry, index }))
    .sort(
      (a, b) =>
        (commits.get(b.entry.personId) ?? 0) -
          (commits.get(a.entry.personId) ?? 0) || a.index - b.index,
    )
    .map(({ entry }) => entry);
  const shown = ranked.slice(0, LEGEND_LIMIT);
  const rest = ranked.slice(LEGEND_LIMIT);
  return (
    <ul className="flex flex-wrap gap-x-3 gap-y-1 px-3 pt-2 text-[12px] text-content/50">
      {shown.map((l) => (
        <li key={l.personId} className="flex items-center gap-1.5">
          <span
            aria-hidden
            className="size-2 rounded-sm"
            style={{ backgroundColor: l.color }}
          />
          {l.name}
        </li>
      ))}
      {rest.length > 0 ? (
        <li
          title={rest.map((l) => l.name).join(", ")}
          className="rounded-full bg-content/8 px-1.5 text-content/60"
        >
          {`+${rest.length} more`}
        </li>
      ) : null}
    </ul>
  );
}

function Lines({ row }: { row: StatsRow }) {
  return (
    <span className="tabular-nums">
      <span className="text-diff-add-fg">+{formatCompact(row.added)}</span>{" "}
      <span className="text-diff-del-fg">-{formatCompact(row.deleted)}</span>
    </span>
  );
}

const TH = "px-3 py-2 text-[12px] font-medium text-content/50";
const TD = "px-3 py-2 align-middle";

function ContributorRow({
  row,
  index,
  expanded,
  now,
  onToggle,
  onAskAgent,
}: {
  row: StatsRow;
  index: number;
  expanded: boolean;
  now: Date;
  onToggle: () => void;
  onAskAgent?: (scope: SnapshotScope) => void;
}) {
  return (
    <Fragment>
      <tr
        onClick={onToggle}
        className="group cursor-pointer border-t border-stroke hover:bg-content/3"
      >
        <td className={`${TD} tabular-nums text-content/50`}>{index + 1}</td>
        <td className={TD}>
          <button
            type="button"
            aria-expanded={expanded}
            aria-controls={expanded ? `contributors-ids-${index}` : undefined}
            aria-label={`${row.name}, ${row.identities.length} ${row.identities.length === 1 ? "identity" : "identities"}`}
            onClick={(e) => {
              e.stopPropagation();
              onToggle();
            }}
            className="flex items-center gap-2 text-left outline-none focus-visible:outline-2 focus-visible:outline-accent"
          >
            <Avatar name={row.name} color={row.color} />
            <span className="min-w-0 truncate font-medium">{row.name}</span>
            {row.identities.length > 1 ? (
              <span className="rounded-full border border-dashed border-content/20 px-1.5 text-[11px] text-content/50">
                {row.identities.length} identities
              </span>
            ) : null}
          </button>
        </td>
        <td className={`${TD} text-right tabular-nums`}>{row.commits}</td>
        <td className={`${TD} text-right`}>
          <Lines row={row} />
        </td>
        <td className={`${TD} text-right tabular-nums`}>{row.files}</td>
        <td className={`${TD} text-right tabular-nums`}>
          {row.aiAssistedPct}%
        </td>
        <td className={TD}>
          <div className="flex items-center gap-2">
            <div className="h-1.5 w-24 overflow-hidden rounded-full bg-content/10">
              <div
                className="h-full rounded-full"
                style={{
                  width: `${Math.min(100, row.sharePct)}%`,
                  backgroundColor: row.color,
                }}
              />
            </div>
            <span className="w-8 text-[11px] tabular-nums text-content/50">
              {Math.round(row.sharePct)}%
            </span>
          </div>
        </td>
        <td className={`${TD} whitespace-nowrap text-right text-content/50`}>
          {row.lastActive ? relativeTime(row.lastActive, now) : ""}
        </td>
        <td className={`${TD} text-right`}>
          {onAskAgent ? (
            <button
              type="button"
              aria-label={`Ask Agent about ${row.name}`}
              onClick={(e) => {
                e.stopPropagation();
                onAskAgent({ kind: "person", personId: row.personId });
              }}
              className="rounded-full border border-content/10 px-2 py-0.5 text-[11px] text-accent opacity-0 transition-opacity hover:bg-content/5 focus-visible:opacity-100 group-hover:opacity-100"
            >
              Ask
            </button>
          ) : null}
        </td>
      </tr>
      {expanded ? (
        <tr
          id={`contributors-ids-${index}`}
          data-identities={row.personId}
          className="bg-content/3"
        >
          <td />
          <td colSpan={8} className="px-3 py-2 text-[12px] text-content/50">
            <ul className="flex flex-col gap-0.5 font-mono">
              {row.identities.length === 0 ? (
                <li>No commit identities</li>
              ) : null}
              {row.identities.map((id) => (
                <li key={id}>{id}</li>
              ))}
            </ul>
          </td>
        </tr>
      ) : null}
    </Fragment>
  );
}

export function StatsTab({ model, onAskAgent }: Props) {
  const { stats } = model;
  const [open, setOpen] = useState<string | null>(null);
  const now = new Date();
  return (
    <div className="flex flex-col gap-3.5">
      <KpiCards model={model} />

      <section className="overflow-hidden rounded-xl border border-stroke bg-content/3">
        <h3 className="border-b border-stroke px-3 py-2.5 text-[13px] font-semibold">
          Commit activity{" "}
          <span className="font-normal text-content/50">
            {`per ${stats.granularity}, stacked by contributor`}
          </span>
        </h3>
        <Legend stats={stats} />
        <CommitChart stats={stats} />
      </section>

      <section className="overflow-hidden rounded-xl border border-stroke bg-content/3">
        <h3 className="border-b border-stroke px-3 py-2.5 text-[13px] font-semibold">
          Contributors{" "}
          <span className="font-normal text-content/50">
            click a row to see its identities
          </span>
        </h3>
        {stats.rows.length === 0 ? (
          <p className="px-3 py-6 text-center text-[12px] text-content/50">
            No contributors in this period.
          </p>
        ) : (
          <ScrollX label="Contributors table">
            <table className="w-full border-collapse text-[13px]">
              <thead>
                <tr className="text-left">
                  <th className={TH}>#</th>
                  <th className={TH}>Contributor</th>
                  <th className={`${TH} text-right`}>Commits</th>
                  <th className={`${TH} text-right`}>+ / -</th>
                  <th className={`${TH} text-right`}>Files</th>
                  <th className={`${TH} text-right`}>AI-assisted</th>
                  <th className={TH}>Share</th>
                  <th className={`${TH} text-right`}>Last active</th>
                  <th className={TH} />
                </tr>
              </thead>
              <tbody>
                {stats.rows.map((row, i) => (
                  <ContributorRow
                    key={row.personId}
                    row={row}
                    index={i}
                    expanded={open === row.personId}
                    now={now}
                    onToggle={() =>
                      setOpen(open === row.personId ? null : row.personId)
                    }
                    onAskAgent={onAskAgent}
                  />
                ))}
              </tbody>
            </table>
          </ScrollX>
        )}
      </section>
    </div>
  );
}
