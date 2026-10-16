import { useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Sparkles, X } from "../../../shared/ui/icons";
import { DEFAULT_CONFIG, CONFIG_PATH } from "../model/config";
import { formatScore } from "../model/format";
import type { LeaderboardRow } from "../model/leaderboard";
import { colorFor } from "../model/palette";
import type { ContributorsModel } from "../model/pipeline";
import { SCORING } from "../model/scoring";
import type { SnapshotScope } from "../model/snapshot";
import type { ContributorsConfig } from "../model/types";

const MIN_WEIGHT = 0;
const MAX_WEIGHT = 5;

type WeightKey = keyof ContributorsConfig["weights"];

const WEIGHT_FIELDS: { key: WeightKey; label: string }[] = [
  { key: "delivery", label: "Delivery" },
  { key: "care", label: "Care" },
  { key: "review", label: "Review" },
  { key: "consistency", label: "Consistency" },
  { key: "breadth", label: "Breadth" },
];

const BAR_COLORS: Record<WeightKey, string> = {
  delivery: colorFor(0),
  care: colorFor(2),
  review: colorFor(1),
  consistency: colorFor(4),
  breadth: colorFor(5),
};

const pct = (fraction: number) => `${Math.round(fraction * 100)}%`;

/** A typed weight kept inside the allowed range; an empty or invalid entry keeps `fallback`. */
function clampWeight(text: string, fallback: number): number {
  const value = text.trim() === "" ? Number.NaN : Number(text);
  if (!Number.isFinite(value)) return fallback;
  const clamped = Math.min(MAX_WEIGHT, Math.max(MIN_WEIGHT, value));
  return Math.round(clamped * 100) / 100;
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section>
      <h4 className="mb-1.5 mt-4 text-[13px] font-semibold">{title}</h4>
      <div className="flex flex-col gap-1.5 text-[12px] leading-relaxed text-content/60">
        {children}
      </div>
    </section>
  );
}

function Formula({ children }: { children: string }) {
  return (
    <pre className="overflow-x-auto whitespace-pre rounded-md border border-stroke bg-content/3 px-2.5 py-2 font-mono text-[11.5px] text-content">
      {children}
    </pre>
  );
}

function Breakdown({
  row,
  weights,
}: {
  row: LeaderboardRow;
  weights: ContributorsConfig["weights"];
}) {
  const parts = row.parts;
  const max = Math.max(
    parts.delivery,
    parts.care,
    parts.review,
    parts.consistency,
    parts.breadth,
    1,
  );
  const survival =
    row.survival === null
      ? "no 30-day data yet"
      : `survival ${pct(row.survival)} at 30 days`;
  const bar = (key: WeightKey, label: string) => (
    <div
      key={key}
      data-part={key}
      className="grid grid-cols-[90px_1fr_44px] items-center gap-1.5 text-[12px]"
    >
      <span>
        {label}
        {weights[key] !== 1 ? (
          <span className="ml-1 text-[11px] text-content/45">
            {`×${weights[key]}`}
          </span>
        ) : null}
      </span>
      <div className="h-1.5 overflow-hidden rounded-full bg-content/10">
        <div
          className="h-full rounded-full"
          style={{
            width: `${(parts[key] / max) * 100}%`,
            backgroundColor: BAR_COLORS[key],
          }}
        />
      </div>
      <span className="text-right tabular-nums">{formatScore(parts[key])}</span>
    </div>
  );
  return (
    <div className="mt-1 flex flex-col gap-1">
      {bar("delivery", "Delivery")}
      <div
        data-part="quality"
        title="Already included in the Delivery number above"
        className="grid grid-cols-[90px_1fr] items-center gap-1.5 text-[12px]"
      >
        <span>× Quality</span>
        <span className="text-content/50">
          <span className="mr-1.5 rounded-full border border-stroke px-1.5 text-[11px] tabular-nums text-content">
            {`×${row.quality.toFixed(2)}`}
          </span>
          {`(${survival})`}
        </span>
      </div>
      {bar("care", "Care")}
      {bar("review", "Review")}
      {bar("consistency", "Consistency")}
      {bar("breadth", "Breadth")}
    </div>
  );
}

function WeightsEditor({
  config,
  locked,
  onSave,
}: {
  config: ContributorsConfig;
  /** The config file cannot be read, so nothing may be written over it. */
  locked: boolean;
  onSave: (weights: ContributorsConfig["weights"]) => void;
}) {
  const [draft, setDraft] = useState<Record<WeightKey, string>>(() => ({
    delivery: String(config.weights.delivery),
    care: String(config.weights.care),
    review: String(config.weights.review),
    consistency: String(config.weights.consistency),
    breadth: String(config.weights.breadth),
  }));
  const next = (): ContributorsConfig["weights"] => ({
    delivery: clampWeight(draft.delivery, config.weights.delivery),
    care: clampWeight(draft.care, config.weights.care),
    review: clampWeight(draft.review, config.weights.review),
    consistency: clampWeight(draft.consistency, config.weights.consistency),
    breadth: clampWeight(draft.breadth, config.weights.breadth),
  });
  const unchanged = WEIGHT_FIELDS.every(
    ({ key }) => next()[key] === config.weights[key],
  );
  const set = (key: WeightKey, text: string) =>
    setDraft((all) => ({ ...all, [key]: text }));
  return (
    <div
      data-weights-editor
      className="mt-3 flex flex-col gap-2 rounded-md border border-stroke bg-content/3 px-3 py-2.5"
    >
      <p className="text-[12px] text-content/60">
        Each dimension is multiplied by its weight, from {MIN_WEIGHT} to{" "}
        {MAX_WEIGHT}. The default is 1.
      </p>
      <div className="grid grid-cols-[1fr_72px] items-center gap-x-3 gap-y-1.5 text-[12px]">
        {WEIGHT_FIELDS.map(({ key, label }) => (
          <label key={key} className="contents">
            <span>{label}</span>
            <input
              type="number"
              inputMode="decimal"
              min={MIN_WEIGHT}
              max={MAX_WEIGHT}
              step={0.1}
              aria-label={`${label} weight`}
              disabled={locked}
              value={draft[key]}
              onChange={(e) => set(key, e.target.value)}
              className="w-full rounded-md border border-content/10 bg-transparent px-2 py-1 text-right tabular-nums text-content outline-none focus:border-accent/60"
            />
          </label>
        ))}
      </div>
      <div className="flex items-center gap-2">
        <button
          type="button"
          disabled={unchanged || locked}
          onClick={() => {
            const weights = next();
            setDraft({
              delivery: String(weights.delivery),
              care: String(weights.care),
              review: String(weights.review),
              consistency: String(weights.consistency),
              breadth: String(weights.breadth),
            });
            onSave(weights);
          }}
          className="rounded-md bg-accent px-2.5 py-1 text-[12px] font-medium text-white hover:opacity-90 disabled:opacity-40"
        >
          Save
        </button>
        <button
          type="button"
          disabled={locked}
          onClick={() =>
            setDraft({
              delivery: String(DEFAULT_CONFIG.weights.delivery),
              care: String(DEFAULT_CONFIG.weights.care),
              review: String(DEFAULT_CONFIG.weights.review),
              consistency: String(DEFAULT_CONFIG.weights.consistency),
              breadth: String(DEFAULT_CONFIG.weights.breadth),
            })
          }
          className="rounded-md border border-content/10 px-2.5 py-1 text-[12px] text-content/70 hover:bg-content/10 hover:text-content disabled:opacity-40 disabled:hover:bg-transparent"
        >
          Reset to defaults
        </button>
      </div>
    </div>
  );
}

export type FormulaSidebarProps = {
  model: ContributorsModel;
  id?: string;
  onClose: () => void;
  onAskAgent?: (scope: SnapshotScope) => void;
  /** Offers the weights editor; the page reports a write that fails. */
  onChangeConfig?: (config: ContributorsConfig) => void;
  /** The config file cannot be read: the editor stays visible but disabled. */
  editLocked?: boolean;
  /** Renders into this element (the page body) instead of in place. */
  host?: HTMLElement | null;
};

/**
 * Explains the score and the viewer's own breakdown. The copy is built from
 * the constants the scoring uses, so it cannot drift from the numbers.
 */
export function FormulaSidebar({
  model,
  id,
  onClose,
  onAskAgent,
  onChangeConfig,
  editLocked = false,
  host,
}: FormulaSidebarProps) {
  const panel = useRef<HTMLElement>(null);
  const [entered, setEntered] = useState(false);
  const [editing, setEditing] = useState(false);
  const close = useRef(onClose);
  close.current = onClose;

  useEffect(() => {
    panel.current?.focus();
    const frame = requestAnimationFrame(() => setEntered(true));
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      e.stopPropagation();
      close.current();
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("keydown", onKey, true);
    };
  }, []);

  const { config, raw } = model;
  const me = model.leaderboard.rows.find((r) => r.isMe);
  const reason = raw.ghNote?.trim().replace(/[.\s]+$/, "");
  const periodWord =
    model.period.kind === "7d"
      ? "in the last 7 days"
      : model.period.kind === "all"
        ? `over ${model.period.label}`
        : `in ${model.period.label}`;
  const customWeights = WEIGHT_FIELDS.filter(
    ({ key }) => config.weights[key] !== 1,
  );
  const canEdit = !!onChangeConfig;

  const content = (
    <aside
      ref={panel}
      id={id}
      role="complementary"
      aria-label="How the score is calculated"
      tabIndex={-1}
      className={`pointer-events-auto h-full w-full overflow-y-auto border-l border-stroke bg-background-base px-4 py-3.5 shadow-[-12px_0_24px_rgba(0,0,0,0.25)] outline-none transition-transform duration-200 motion-reduce:transition-none ${
        entered ? "translate-x-0" : "translate-x-full"
      } ${host ? "" : "absolute inset-y-0 right-0 z-10 w-[400px] max-w-full"}`}
    >
      <div className="flex items-center">
        <h3 className="flex-1 text-[14px] font-semibold">
          How the score is calculated
        </h3>
        <button
          type="button"
          aria-label="Close"
          onClick={onClose}
          className="rounded-md p-1 text-content/50 hover:bg-content/8 hover:text-content"
        >
          <X className="size-3.5" strokeWidth={1.75} />
        </button>
      </div>
      <p className="mt-2 text-[12px] leading-relaxed text-content/60">
        Scores reward impact, not busyness. Every part grows slowly (log scale)
        and has a cap, so splitting work into tiny commits or padding lines does
        not pay.
      </p>

      <Section
        title={
          me
            ? `Your score ${periodWord}: ${formatScore(me.score)}`
            : `Your score ${periodWord}`
        }
      >
        {me ? (
          <Breakdown row={me} weights={config.weights} />
        ) : (
          <p>{`No activity for you ${periodWord} yet`}</p>
        )}
        {customWeights.length > 0 ? (
          <p>
            Custom weights are active:{" "}
            {customWeights
              .map(({ key, label }) => `${label} ×${config.weights[key]}`)
              .join(", ")}
            .
          </p>
        ) : null}
      </Section>

      <Section title="The formula">
        <Formula>
          {
            "Score = Delivery × Quality + Care\n      + Review + Consistency + Breadth"
          }
        </Formula>
      </Section>

      <Section title="Delivery: merged pull requests">
        <p>
          Each merged PR is one unit, whether it was squashed, rebased or
          merged. Points come from the final diff that landed, plus how much
          iteration it took.
        </p>
        <Formula>
          {[
            "size      = log2(1 + added + 0.5 × deleted)",
            `iteration = ${SCORING.ITER_K} × log2(1 + commits in PR)  (max ${SCORING.ITER_CAP})`,
            `PR points = ${SCORING.PR_BASE} + ${SCORING.SIZE_K} × size + iteration     (max ${SCORING.PR_CAP})`,
          ].join("\n")}
        </Formula>
        <ul className="list-disc pl-4">
          <li>
            <b className="text-content">Squash-merged PRs keep their effort.</b>{" "}
            The commits on the branch count as iteration, and every day you
            committed on the branch counts as an active day.
          </li>
          <li>
            Closed PRs that were never merged earn no Delivery points, but their
            days still count toward Consistency.
          </li>
        </ul>
      </Section>

      <Section
        title={`Quality: ×${SCORING.QUALITY_MIN} to ×${SCORING.QUALITY_MAX}`}
      >
        <p>
          Based on how much of your code is still alive 30 days later. A
          reverted PR loses its Delivery points, with no extra penalty.
        </p>
        <Formula>
          {`quality = ${SCORING.QUALITY_BASE} + ${SCORING.QUALITY_SLOPE} × survival`}
        </Formula>
      </Section>

      <Section title="Care: bonus on a PR">
        <ul className="list-disc pl-4">
          <li>{`+${pct(SCORING.CARE_TEST)} if it adds or updates tests`}</li>
          <li>{`+${pct(SCORING.CARE_DOCS)} if it updates docs`}</li>
          <li>
            {`+${pct(SCORING.CARE_NET_NEGATIVE)} if it deletes more than it adds`}
          </li>
          <li>{`+${pct(SCORING.CARE_FIX)} if it fixes a bug`}</li>
        </ul>
      </Section>

      <Section title="Review">
        <p>
          {`${SCORING.REVIEW_PR} points per PR you review for someone else, +${SCORING.REVIEW_COMMENT} per meaningful comment (max ${SCORING.REVIEW_COMMENT_CAP} per PR). Reviewing your own PR earns nothing.`}
        </p>
        {raw.ghAvailable ? null : (
          <p role="status" className="text-amber-700 dark:text-amber-400">
            {`Review points are not counted because GitHub data is unavailable${reason ? `: ${reason}` : ""}.`}
          </p>
        )}
      </Section>

      <Section title="Consistency">
        <p>
          {`${SCORING.ACTIVE_DAY} points per active day, plus ${SCORING.STREAK_K} per day of streak (max ${SCORING.STREAK_CAP}). Weekends never break a streak.`}
        </p>
        <p>
          Consistency grows with the length of the period; Breadth is capped, so
          in long periods it weighs less.
        </p>
      </Section>

      <Section title="Breadth">
        <p>
          {`${SCORING.MODULE_PTS} points per module where you changed at least ${SCORING.MODULE_MIN_EFF} lines (max ${SCORING.MODULE_CAP} modules).`}
        </p>
      </Section>

      <Section title="AI agents">
        <p>
          Driving an agent is real work. Commits co-authored by Claude, Codex,
          Copilot or any other agent count <b className="text-content">100%</b>{" "}
          for the human. If an agent is the commit author, the points go to the
          PR author or the committer. There is no AI bonus or penalty, but
          Quality applies to all code alike. &quot;AI-assisted %&quot; is shown
          for information only.
        </p>
      </Section>

      <Section title="What is ignored">
        <p>
          Merge commits, lockfiles, generated files, whitespace-only changes,
          renames, duplicate cherry-picks, and bots such as dependabot or CI
          runners.
        </p>
        {config.ignore.length > 0 ? (
          <>
            <p>File patterns ignored for this project:</p>
            <ul
              data-ignore-globs
              className="flex flex-wrap gap-1 font-mono text-[11px]"
            >
              {config.ignore.map((glob) => (
                <li
                  key={glob}
                  className="rounded border border-stroke bg-content/3 px-1.5 py-0.5 text-content/70"
                >
                  <code>{glob}</code>
                </li>
              ))}
            </ul>
          </>
        ) : (
          <p>No file patterns are ignored for this project.</p>
        )}
      </Section>

      <div className="mt-4 rounded-md border border-stroke border-l-[3px] border-l-accent px-2.5 py-2 text-[12px] leading-relaxed text-content/60">
        This board is for motivation, not performance review. Roles differ:
        someone who reviews and mentors a lot may ship less code.
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        {onAskAgent ? (
          <button
            type="button"
            disabled={!me}
            onClick={() => {
              if (me) onAskAgent({ kind: "me", personId: me.personId });
            }}
            className="flex items-center gap-1.5 rounded-md bg-accent px-2.5 py-1 text-[12px] font-medium text-white hover:opacity-90 disabled:opacity-40"
          >
            <Sparkles className="size-3.5" strokeWidth={1.75} />
            Ask Agent about my score
          </button>
        ) : null}
        {canEdit ? (
          <button
            type="button"
            aria-expanded={editing}
            disabled={editLocked}
            onClick={() => setEditing((on) => !on)}
            className="rounded-md border border-content/10 px-2.5 py-1 text-[12px] text-content/70 hover:bg-content/10 hover:text-content disabled:opacity-40 disabled:hover:bg-transparent"
          >
            Customize weights
          </button>
        ) : null}
        {canEdit ? (
          <span className="text-[11px] text-content/45">
            Saved in {CONFIG_PATH}
          </span>
        ) : null}
      </div>
      {canEdit && editing ? (
        <WeightsEditor
          config={config}
          locked={editLocked}
          onSave={(weights) => onChangeConfig?.({ ...config, weights })}
        />
      ) : null}
    </aside>
  );
  return host ? createPortal(content, host) : content;
}
