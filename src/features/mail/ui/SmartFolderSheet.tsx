import { useEffect, useMemo, useRef, useState } from "react";
import { normalizeHex } from "../../../shared/lib/colorUtils";
import {
  ColorPickerPopover,
  ColorSwatchRow,
} from "../../../shared/ui/ColorPickerPopover";
import { Loader, Plus, X } from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { SearchableSelect } from "../../../shared/ui/SearchableSelect";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { TAB_GROUP_COLORS } from "../../workspace/model/tabGroups";
import { Toggle } from "../../settings/ui/primitives";
import {
  mailFolderDelete,
  mailFolderPreview,
  mailFolderUpsert,
} from "../model/mail";
import {
  FIELD_LABELS,
  RULE_FIELDS,
  MAX_LIST_VALUES,
  MAX_RULES,
  SEARCH_DEBOUNCE_MS,
  defaultOp,
  editorOps,
  hasClockRelativeRule,
  isFlagField,
  opLabel,
  ruleProblem,
  normaliseRule,
  valueList,
  valueText,
} from "../model/mailQuery";
import type {
  FolderRow,
  MatchMode,
  Rule,
  RuleField,
  RuleOp,
} from "../model/mailTypes";

/** Names the value fields can suggest; plain text is always accepted. */
export type RuleSuggestions = {
  mailboxes: string[];
  domains: string[];
  sources: string[];
};

type Props = {
  /** The folder being edited; null for a new one. */
  folder: FolderRow | null;
  /** Pre-fills a new folder from the filters it was saved from. */
  initial?: { rules: Rule[]; matchMode: MatchMode } | null;
  suggestions: RuleSuggestions;
  onClose: () => void;
  onSaved: (folder: FolderRow) => void;
  onDeleted: (id: number) => void;
};

type Row = { key: number; rule: Rule };

const FIELD_OPTIONS = RULE_FIELDS.map((value) => ({
  value,
  label: FIELD_LABELS[value],
}));

const INPUT =
  "h-7 w-full min-w-0 rounded-md border border-content/10 bg-transparent px-2 text-[12px] text-content outline-none placeholder:text-content/35 focus:border-accent/50";
const LABEL = "text-[11px] font-medium text-content/50";

const blankRule = (field: RuleField = "from"): Rule => {
  const op = defaultOp(field);
  return {
    field,
    op,
    value: op === "contains_any" ? [] : "",
    negate: false,
  };
};

/** Rust trims nothing it keeps, so what is saved is what the count ran on. */
function wire(rule: Rule): Rule {
  if (isFlagField(rule.field)) return { ...rule, value: "" };
  if (rule.op === "contains_any") {
    return {
      ...rule,
      value: valueList(rule.value)
        .map((v) => v.trim())
        .filter((v) => v !== ""),
    };
  }
  return { ...rule, value: valueText(rule.value).trim() };
}

function TokenInput({
  values,
  onChange,
  label,
}: {
  values: string[];
  onChange: (values: string[]) => void;
  label: string;
}) {
  const full = values.length >= MAX_LIST_VALUES;
  const [draft, setDraft] = useState("");
  const commit = () => {
    const token = draft.trim();
    setDraft("");
    if (token !== "" && !full && !values.includes(token))
      onChange([...values, token]);
  };
  return (
    <div className="flex min-h-7 min-w-0 flex-wrap items-center gap-1 rounded-md border border-content/10 px-1.5 py-0.5 focus-within:border-accent/50">
      {values.map((value) => (
        <span
          key={value}
          className="flex items-center gap-1 rounded-full border border-content/10 bg-content/5 px-1.5 text-[11px] text-content/80"
        >
          {value}
          <button
            type="button"
            aria-label={`Remove ${value}`}
            onClick={() => onChange(values.filter((v) => v !== value))}
            className="text-content/50 hover:text-content"
          >
            <X className="size-2.5" strokeWidth={2} />
          </button>
        </span>
      ))}
      <input
        aria-label={label}
        value={draft}
        placeholder={
          full
            ? `At most ${MAX_LIST_VALUES} values`
            : values.length === 0
              ? "Type, then Enter"
              : ""
        }
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            commit();
          } else if (e.key === "Backspace" && draft === "") {
            onChange(values.slice(0, -1));
          }
        }}
        className="h-5 min-w-16 flex-1 bg-transparent text-[12px] text-content outline-none placeholder:text-content/35"
      />
    </div>
  );
}

function ValueEditor({
  rule,
  onChange,
}: {
  rule: Rule;
  onChange: (value: Rule["value"]) => void;
}) {
  const label = `${FIELD_LABELS[rule.field]} value`;
  if (isFlagField(rule.field)) {
    return <span className="text-[12px] text-content/45">is true</span>;
  }
  if (rule.op === "contains_any") {
    return (
      <TokenInput
        label={label}
        values={valueList(rule.value)}
        onChange={onChange}
      />
    );
  }
  if (rule.field === "date" && rule.op !== "within_days") {
    // The native date input is the YYYY-MM-DD the backend and `after:` use.
    return (
      <input
        type="date"
        aria-label={label}
        value={valueText(rule.value)}
        onChange={(e) => onChange(e.target.value)}
        className={INPUT}
      />
    );
  }
  if (rule.field === "date") {
    return (
      <input
        type="number"
        min={1}
        max={36500}
        step={1}
        aria-label={label}
        value={valueText(rule.value)}
        onChange={(e) => onChange(e.target.value)}
        className={INPUT}
      />
    );
  }
  const listId =
    rule.field === "mailbox"
      ? "mail-rule-mailboxes"
      : rule.field === "domain"
        ? "mail-rule-domains"
        : rule.field === "account"
          ? "mail-rule-sources"
          : undefined;
  return (
    <input
      aria-label={label}
      list={rule.op === "is" ? listId : undefined}
      value={valueText(rule.value)}
      onChange={(e) => onChange(e.target.value)}
      className={`${INPUT} ${rule.op === "matches" ? "font-mono" : ""}`}
    />
  );
}

export function SmartFolderSheet({
  folder,
  initial = null,
  suggestions,
  onClose,
  onSaved,
  onDeleted,
}: Props) {
  const nextKey = useRef(0);
  const keyed = (rules: Rule[]): Row[] =>
    rules.map((rule) => ({
      key: nextKey.current++,
      rule: normaliseRule(rule),
    }));
  const [name, setName] = useState(folder?.name ?? "");
  const [color, setColor] = useState<string | null>(folder?.color ?? null);
  const [customOpen, setCustomOpen] = useState(false);
  const [matchMode, setMatchMode] = useState<MatchMode>(
    folder?.matchMode ?? initial?.matchMode ?? "all",
  );
  const [rows, setRows] = useState<Row[]>(() => {
    const start = folder?.rules ?? initial?.rules ?? [];
    return keyed(start.length > 0 ? start : [blankRule()]);
  });
  const [hide, setHide] = useState(folder?.hideFromMailbox ?? false);
  const [countUnread, setCountUnread] = useState(folder?.countUnread ?? true);
  const [count, setCount] = useState<number | "pending" | null>(null);
  const [countError, setCountError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const rules = useMemo(() => rows.map((r) => wire(r.rule)), [rows]);
  const problems = rules.map(ruleProblem);
  // A clock-relative folder never hides: its stored members go stale, and an
  // aged-out thread would be in neither place.
  const hideBlocked = hasClockRelativeRule(rules);
  const complete = rules.length > 0 && problems.every((p) => p === null);

  // The count is the same query the list runs, in the editor's current mode.
  // Paused while typing: it is a real query against the local database.
  const previewKey = JSON.stringify([rules, matchMode]);
  useEffect(() => {
    if (!complete) {
      setCount(null);
      setCountError(null);
      return;
    }
    let stale = false;
    setCount("pending");
    const timer = setTimeout(() => {
      mailFolderPreview(rules, matchMode)
        .then((result) => {
          if (stale) return;
          setCount(result.count);
          setCountError(null);
        })
        .catch((err) => {
          if (stale) return;
          setCount(null);
          setCountError(err instanceof Error ? err.message : String(err));
        });
    }, SEARCH_DEBOUNCE_MS);
    return () => {
      stale = true;
      clearTimeout(timer);
    };
    // previewKey is the identity of rules and matchMode.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewKey, complete]);

  const update = (key: number, patch: (rule: Rule) => Rule) =>
    setRows((current) =>
      current.map((r) => (r.key === key ? { ...r, rule: patch(r.rule) } : r)),
    );

  const changeField = (rule: Rule, field: RuleField): Rule => {
    if (field === rule.field) return rule;
    const ops = editorOps(field);
    const op = ops.includes(rule.op) ? rule.op : defaultOp(field);
    // A value for one kind of field means nothing on another (a date on a
    // sender), so only text-to-text keeps it.
    const keep =
      !isFlagField(field) && field !== "date" && rule.field !== "date";
    return {
      ...rule,
      field,
      op,
      value: !keep
        ? op === "contains_any"
          ? []
          : ""
        : op === "contains_any"
          ? valueList(rule.value)
          : valueText(rule.value),
    };
  };

  const changeOp = (rule: Rule, op: RuleOp): Rule => ({
    ...rule,
    op,
    value:
      op === "contains_any" ? valueList(rule.value) : valueText(rule.value),
  });

  const colorIndex = color ? TAB_GROUP_COLORS.indexOf(color as never) : -1;
  const customColor = color && colorIndex < 0 ? color : null;
  const canSave = name.trim() !== "" && complete && !saving;

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      const saved = await mailFolderUpsert(
        {
          id: folder?.id ?? null,
          name: name.trim(),
          color,
          sortOrder: folder?.sortOrder ?? null,
          matchMode,
          hideFromMailbox: hide && !hideBlocked,
          countUnread,
        },
        rules,
      );
      onSaved(saved);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setSaving(false);
    }
  };

  const remove = async () => {
    if (!folder) return;
    setSaving(true);
    setError(null);
    try {
      await mailFolderDelete(folder.id);
      onDeleted(folder.id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setSaving(false);
    }
  };

  return (
    <Modal
      title={folder ? "Edit smart folder" : "New smart folder"}
      description="Mail that arrives later is filed automatically."
      size="md"
      width="w-[min(760px,calc(100vw-24px))]"
      fitViewport
      onClose={onClose}
    >
      <div className="flex flex-col gap-3 px-4 pt-3 pb-3">
        <div className="flex flex-col gap-1">
          <label htmlFor="smart-folder-name" className={LABEL}>
            Name
          </label>
          <input
            id="smart-folder-name"
            autoFocus
            maxLength={80}
            value={name}
            onChange={(e) => setName(e.target.value)}
            className={INPUT}
          />
        </div>

        <div className="flex flex-col gap-1">
          <span className={LABEL}>Colour</span>
          <ColorSwatchRow
            colors={TAB_GROUP_COLORS}
            colorIndex={color === null ? 0 : colorIndex}
            customColor={customColor}
            customPickerOpen={customOpen}
            className="flex items-center gap-1"
            onPickIndex={(index) => {
              setCustomOpen(false);
              setColor(index === 0 ? null : TAB_GROUP_COLORS[index]!);
            }}
            onToggleCustom={() => setCustomOpen((open) => !open)}
          />
          {customOpen ? (
            <ColorPickerPopover
              value={normalizeHex(color ?? TAB_GROUP_COLORS[0])}
              onChange={setColor}
            />
          ) : null}
        </div>

        <div className="flex flex-col gap-1">
          <span className={LABEL}>Match</span>
          <div role="radiogroup" aria-label="Match" className="flex gap-1.5">
            {(
              [
                ["all", "All conditions"],
                ["any", "Any condition"],
              ] as const
            ).map(([mode, text]) => (
              <button
                key={mode}
                type="button"
                role="radio"
                aria-checked={matchMode === mode}
                onClick={() => setMatchMode(mode)}
                className={`rounded-full border px-2.5 py-0.5 text-[11px] focus-visible:outline-2 focus-visible:outline-accent ${
                  matchMode === mode
                    ? "border-accent/40 bg-accent/10 text-content"
                    : "border-content/10 bg-content/5 text-content/70 hover:bg-content/10"
                }`}
              >
                {text}
              </button>
            ))}
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <span className="text-[10px] font-medium uppercase tracking-wide text-content/40">
            Conditions
          </span>
          {rows.map(({ key, rule }, index) => {
            const problem = problems[index];
            const filled =
              rule.op === "contains_any"
                ? valueList(rule.value).length > 0
                : valueText(rule.value).trim() !== "";
            return (
              <div key={key} data-rule-row>
                <div className="grid grid-cols-[40px_112px_132px_minmax(0,1fr)_26px] items-center gap-1.5">
                  <button
                    type="button"
                    aria-pressed={rule.negate}
                    title="Match when the condition is NOT true"
                    onClick={() =>
                      update(key, (r) => ({ ...r, negate: !r.negate }))
                    }
                    className={`h-7 rounded-md border text-[10px] font-medium focus-visible:outline-2 focus-visible:outline-accent ${
                      rule.negate
                        ? "border-accent/40 bg-accent/10 text-content"
                        : "border-content/10 text-content/45 hover:bg-content/10"
                    }`}
                  >
                    NOT
                  </button>
                  <SearchableSelect
                    label="Field"
                    value={rule.field}
                    options={FIELD_OPTIONS}
                    searchable={false}
                    onChange={(value) =>
                      update(key, (r) => changeField(r, value as RuleField))
                    }
                  />
                  {isFlagField(rule.field) ? (
                    <span />
                  ) : (
                    <SearchableSelect
                      label="Operator"
                      value={rule.op}
                      options={editorOps(rule.field).map((op) => ({
                        value: op,
                        label: opLabel(rule.field, op),
                      }))}
                      searchable={false}
                      onChange={(value) =>
                        update(key, (r) => changeOp(r, value as RuleOp))
                      }
                    />
                  )}
                  <ValueEditor
                    rule={rule}
                    onChange={(value) => update(key, (r) => ({ ...r, value }))}
                  />
                  <button
                    type="button"
                    aria-label="Remove condition"
                    disabled={rows.length === 1}
                    onClick={() =>
                      setRows((current) => current.filter((r) => r.key !== key))
                    }
                    className="grid size-6 place-items-center rounded-md text-content/45 hover:bg-content/10 hover:text-content disabled:opacity-30 disabled:hover:bg-transparent"
                  >
                    <X className="size-3" strokeWidth={1.75} />
                  </button>
                </div>
                {problem && filled ? (
                  <p
                    role="alert"
                    className="mt-0.5 pl-[46px] text-[11px] text-red-400/90"
                  >
                    {problem}
                  </p>
                ) : null}
              </div>
            );
          })}
          <div>
            <SecondaryButton
              disabled={rows.length >= MAX_RULES}
              title={
                rows.length >= MAX_RULES
                  ? `A folder can have at most ${MAX_RULES} conditions`
                  : undefined
              }
              onClick={() =>
                setRows((current) => [...current, ...keyed([blankRule()])])
              }
            >
              <Plus className="size-3" strokeWidth={1.75} />
              Add condition
            </SecondaryButton>
          </div>
          <datalist id="mail-rule-mailboxes">
            {suggestions.mailboxes.map((v) => (
              <option key={v} value={v} />
            ))}
          </datalist>
          <datalist id="mail-rule-domains">
            {suggestions.domains.map((v) => (
              <option key={v} value={v} />
            ))}
          </datalist>
          <datalist id="mail-rule-sources">
            {suggestions.sources.map((v) => (
              <option key={v} value={v} />
            ))}
          </datalist>
        </div>

        <div className="overflow-hidden rounded-xl border border-content/10 bg-content/3">
          <div className="flex items-start gap-4 border-b border-content/5 px-3 py-2.5">
            <div className="min-w-0 flex-1">
              <div className="text-[12px] font-medium text-content">
                Hide matches from All mail and mailboxes
              </div>
              {hideBlocked ? (
                <p
                  id="hide-blocked-reason"
                  className="mt-0.5 text-[11px] leading-relaxed text-content/60"
                >
                  Not available with a "within the last (days)" condition. Such
                  a folder is judged by today's date, so its threads always stay
                  in their mailbox as well.
                </p>
              ) : null}
              <p className="mt-0.5 text-[11px] leading-relaxed text-content/45">
                On: matching threads leave All mail and their mailbox, and stop
                counting as unread there. They appear only in this folder, and
                search still finds them. Off: they stay in their mailbox and
                also show here, like a label.
              </p>
            </div>
            <Toggle
              label="Hide matches from All mail and mailboxes"
              on={hide && !hideBlocked}
              disabled={hideBlocked}
              onChange={setHide}
            />
          </div>
          <div className="flex items-center gap-4 px-3 py-2.5">
            <div className="min-w-0 flex-1 text-[12px] font-medium text-content">
              Count unread in the sidebar
            </div>
            <Toggle
              label="Count unread in the sidebar"
              on={countUnread}
              onChange={setCountUnread}
            />
          </div>
        </div>

        {error ? (
          <p role="alert" className="text-[12px] text-red-400/90">
            {error}
          </p>
        ) : null}
      </div>

      <div className="sticky bottom-0 flex shrink-0 items-center gap-2 border-t border-stroke bg-background-base px-4 py-2.5">
        <span
          role="status"
          className="min-w-0 flex-1 truncate text-[11.5px] text-content/50"
        >
          {count === "pending" ? (
            <>
              <Loader className="mr-1 inline size-3 animate-spin" />
              Counting
            </>
          ) : typeof count === "number" ? (
            <>
              <b className="font-semibold text-accent">{count}</b>{" "}
              {count === 1 ? "thread matches" : "threads match"} now
            </>
          ) : countError ? (
            `Could not count: ${countError}`
          ) : (
            "Complete every condition to see the count"
          )}
        </span>
        {folder ? (
          confirmDelete ? (
            <>
              <span className="text-[11px] text-content/50">
                Mail stays where it is.
              </span>
              <SecondaryButton
                danger
                disabled={saving}
                onClick={() => void remove()}
              >
                Delete folder
              </SecondaryButton>
              <SecondaryButton onClick={() => setConfirmDelete(false)}>
                Keep
              </SecondaryButton>
            </>
          ) : (
            <SecondaryButton danger onClick={() => setConfirmDelete(true)}>
              Delete
            </SecondaryButton>
          )
        ) : null}
        {confirmDelete ? null : (
          <>
            <SecondaryButton onClick={onClose}>Cancel</SecondaryButton>
            <SecondaryButton disabled={!canSave} onClick={() => void save()}>
              {saving ? "Saving" : "Save folder"}
            </SecondaryButton>
          </>
        )}
      </div>
    </Modal>
  );
}
