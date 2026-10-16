import { useEffect, useId, useRef, useState } from "react";
import { Bot, Trophy, Users } from "../../../shared/ui/icons";
import { formatScore } from "../model/format";
import type { LeaderboardRow } from "../model/leaderboard";
import { colorFromMap } from "../model/palette";
import type { ContributorsModel } from "../model/pipeline";
import type { SnapshotScope } from "../model/snapshot";
import type { ContributorsConfig } from "../model/types";
import { Avatar } from "./Avatar";
import { FormulaSidebar } from "./FormulaSidebar";
import { ScrollX } from "./ScrollX";

export type LeaderboardTabProps = {
  model: ContributorsModel;
  onAskAgent?: (scope: SnapshotScope) => void;
  onChangeConfig?: (config: ContributorsConfig) => void;
  /** The config file cannot be read: weights cannot be edited. */
  editLocked?: boolean;
  /** Element of the page body the formula sidebar slides into. */
  sidebarHost?: HTMLElement | null;
};

const TH = "px-3 py-2 text-[12px] font-medium text-content/50";
const TD = "px-3 py-2 align-middle";

const MEDAL_TONE = ["text-amber-400", "text-content/60", "text-orange-400/80"];
const ORDINAL = ["1st", "2nd", "3rd"];

const plural = (n: number, one: string, many = `${one}s`) =>
  `${n} ${n === 1 ? one : many}`;

/** Glyph, tone and explanation for the rank change against the previous period. */
function movementView(movement: number | null) {
  if (movement === null) {
    return {
      glyph: "·",
      tone: "text-content/40",
      title: "Not ranked in the previous period",
    };
  }
  if (movement > 0) {
    return {
      glyph: `▲${movement}`,
      tone: "text-diff-add-fg",
      title: `Up ${plural(movement, "place")} vs previous period`,
    };
  }
  if (movement < 0) {
    return {
      glyph: `▼${-movement}`,
      tone: "text-diff-del-fg",
      title: `Down ${plural(-movement, "place")} vs previous period`,
    };
  }
  return {
    glyph: "·",
    tone: "text-content/40",
    title: "Same rank as the previous period",
  };
}

function YouChip() {
  return (
    <span className="rounded-full border border-accent/40 px-1.5 text-[11px] text-accent">
      you
    </span>
  );
}

function Badges({
  row,
  align,
}: {
  row: LeaderboardRow;
  align: "start" | "center";
}) {
  if (row.badges.length === 0) return null;
  return (
    <ul
      className={`flex flex-wrap gap-1 ${align === "start" ? "justify-start" : "justify-center"}`}
    >
      {row.badges.map((badge) => (
        <li
          key={badge.id}
          data-badge={badge.id}
          className="rounded-full border border-stroke px-1.5 text-[11px] text-content/60"
        >
          {badge.label}
        </li>
      ))}
    </ul>
  );
}

// Visual order is 2nd, 1st, 3rd on wide pages; the DOM keeps rank order so a
// screen reader and a narrow stacked layout read 1st first.
const PODIUM_PLACE = [
  "@min-[560px]/contrib:col-start-2 @min-[560px]/contrib:row-start-1",
  "@min-[560px]/contrib:col-start-1 @min-[560px]/contrib:row-start-1",
  "@min-[560px]/contrib:col-start-3 @min-[560px]/contrib:row-start-1",
];

function Podium({
  rows,
  colors,
}: {
  rows: LeaderboardRow[];
  colors: Record<string, string>;
}) {
  return (
    <ol
      aria-label="Top three"
      data-podium
      className="mb-3.5 grid grid-cols-1 items-end gap-2.5 @min-[560px]/contrib:grid-cols-[1fr_1.15fr_1fr]"
    >
      {rows.slice(0, 3).map((row, i) => (
        <li
          key={row.personId}
          data-rank={row.rank}
          className={`flex flex-col items-center rounded-xl border bg-content/3 px-2.5 text-center ${
            i === 0 ? "border-accent/60 py-5" : "border-stroke py-3.5"
          } ${PODIUM_PLACE[i]}`}
        >
          <div
            className={`flex items-center gap-1.5 text-[12px] font-medium ${MEDAL_TONE[i]}`}
          >
            <Trophy className="size-5" strokeWidth={1.75} />
            {ORDINAL[i]}
          </div>
          <div className="mt-2">
            <Avatar
              name={row.name}
              color={colorFromMap(colors, row.personId)}
              size="lg"
            />
          </div>
          <div className="mt-1.5 flex max-w-full items-center gap-1.5">
            <span className="min-w-0 truncate font-semibold">{row.name}</span>
            {row.isMe ? <YouChip /> : null}
          </div>
          <div className="mt-1 text-[22px] font-bold tabular-nums">
            {formatScore(row.score)}
          </div>
          <div className="mt-1 text-[12px] text-content/50">
            {`${plural(row.mergedPrs, "PR")} (${plural(row.branchCommits, "commit")}) · ${row.streak}d streak`}
          </div>
          <div className="mt-1">
            <Badges row={row} align="center" />
          </div>
        </li>
      ))}
    </ol>
  );
}

function RankingRow({
  row,
  colors,
  compared,
  onAskAgent,
}: {
  row: LeaderboardRow;
  colors: Record<string, string>;
  /** False when there is no previous period (All time): no rank movement to show. */
  compared: boolean;
  onAskAgent?: (scope: SnapshotScope) => void;
}) {
  const move = movementView(row.movement);
  return (
    <tr
      data-rank={row.rank}
      data-me={row.isMe ? "true" : undefined}
      className={`group border-t border-stroke ${
        row.isMe ? "bg-accent/8" : "hover:bg-content/3"
      }`}
    >
      <td className={`${TD} whitespace-nowrap`}>
        <span className="font-semibold tabular-nums">{row.rank}</span>
        {compared ? (
          <span
            title={move.title}
            className={`ml-1.5 inline-block w-7 text-[11px] ${move.tone}`}
          >
            <span aria-hidden>{move.glyph}</span>
            <span className="sr-only">{move.title}</span>
          </span>
        ) : null}
      </td>
      <td className={TD}>
        <div className="flex items-center gap-2">
          <Avatar name={row.name} color={colorFromMap(colors, row.personId)} />
          <span className="min-w-0 truncate font-medium">{row.name}</span>
          {row.isMe ? <YouChip /> : null}
        </div>
      </td>
      <td className={`${TD} text-right font-semibold tabular-nums`}>
        {formatScore(row.score)}
      </td>
      <td className={`${TD} text-right tabular-nums`}>{row.mainCommits}</td>
      <td className={`${TD} whitespace-nowrap text-right tabular-nums`}>
        {row.mergedPrs}{" "}
        <span className="text-content/50">({row.branchCommits})</span>
      </td>
      <td className={`${TD} whitespace-nowrap text-right tabular-nums`}>
        <span className="inline-flex items-center gap-1 text-content/60">
          <Bot className="size-3" strokeWidth={1.75} />
          {Math.round(row.aiAssistedPct)}%
        </span>
      </td>
      <td className={`${TD} text-right tabular-nums`}>{row.streak}d</td>
      <td className={TD}>
        <Badges row={row} align="start" />
      </td>
      <td className={`${TD} text-right`}>
        {onAskAgent ? (
          <button
            type="button"
            aria-label={`Ask Agent about ${row.name}`}
            onClick={() =>
              onAskAgent({ kind: "person", personId: row.personId })
            }
            className="rounded-full border border-content/10 px-2 py-0.5 text-[11px] text-accent opacity-0 transition-opacity hover:bg-content/5 focus-visible:opacity-100 group-hover:opacity-100"
          >
            Ask
          </button>
        ) : null}
      </td>
    </tr>
  );
}

export function LeaderboardTab({
  model,
  onAskAgent,
  onChangeConfig,
  editLocked,
  sidebarHost,
}: LeaderboardTabProps) {
  const { rows } = model.leaderboard;
  const [open, setOpen] = useState(false);
  const how = useRef<HTMLButtonElement>(null);
  const wasOpen = useRef(false);
  const sidebarId = useId();

  // Closing hands the focus back to the link that opened the sidebar.
  useEffect(() => {
    if (wasOpen.current && !open) how.current?.focus();
    wasOpen.current = open;
  }, [open]);

  return (
    <div className={`relative ${open ? "min-h-[40rem]" : ""}`}>
      {rows.length > 0 ? <Podium rows={rows} colors={model.colors} /> : null}
      <section aria-label="Ranking" className="rounded-xl border border-stroke">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-4 pt-3">
          <h2 className="text-[14px] font-semibold">Ranking</h2>
          <span className="text-[12px] text-content/50">
            {`period: ${model.period.label}`}
          </span>
        </div>
        <p className="px-4 pb-2 pt-0.5 text-[12px] text-content/50">
          Rankings are for motivation, not performance review.
        </p>

        {rows.length === 0 ? (
          <div className="flex flex-col items-center gap-2 border-t border-stroke px-4 py-8 text-center">
            <Users className="size-5 text-content/35" strokeWidth={1.75} />
            <p className="text-[13px] font-medium">
              {`No scored activity in ${model.period.label}`}
            </p>
            <p className="text-[12px] text-content/50">
              Merged pull requests, commits and reviews in the period appear
              here.
            </p>
          </div>
        ) : (
          <ScrollX label="Ranking table">
            <table className="w-full border-collapse text-left text-[13px]">
              <thead>
                <tr>
                  <th scope="col" className={`${TH} w-[76px]`}>
                    Rank
                  </th>
                  <th scope="col" className={TH}>
                    Contributor
                  </th>
                  <th scope="col" className={`${TH} text-right`}>
                    Score
                  </th>
                  <th scope="col" className={`${TH} text-right`}>
                    Commits
                  </th>
                  <th scope="col" className={`${TH} text-right`}>
                    PRs <span className="font-normal">(commits)</span>
                  </th>
                  <th scope="col" className={`${TH} text-right`}>
                    AI-assisted
                  </th>
                  <th scope="col" className={`${TH} text-right`}>
                    Streak
                  </th>
                  <th scope="col" className={TH}>
                    Badges
                  </th>
                  <th scope="col" className={TH}>
                    <span className="sr-only">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <RankingRow
                    key={row.personId}
                    row={row}
                    colors={model.colors}
                    compared={model.previousPeriod !== null}
                    onAskAgent={onAskAgent}
                  />
                ))}
              </tbody>
            </table>
          </ScrollX>
        )}

        <p className="flex flex-wrap items-center gap-x-1.5 border-t border-stroke px-4 py-2.5 text-[12px] text-content/55">
          <span>
            Score = Delivery × Quality + Care + Review + Consistency + Breadth.
            {model.raw.ghAvailable
              ? ""
              : " Review is not counted (GitHub data unavailable)."}
          </span>
          <button
            ref={how}
            type="button"
            aria-expanded={open}
            aria-controls={open ? sidebarId : undefined}
            onClick={() => setOpen(true)}
            className="text-accent hover:underline focus-visible:outline-2 focus-visible:outline-accent"
          >
            How is the score calculated?
          </button>
        </p>
      </section>

      {open ? (
        <FormulaSidebar
          id={sidebarId}
          model={model}
          host={sidebarHost}
          onClose={() => setOpen(false)}
          onAskAgent={onAskAgent}
          onChangeConfig={onChangeConfig}
          editLocked={editLocked}
        />
      ) : null}
    </div>
  );
}
