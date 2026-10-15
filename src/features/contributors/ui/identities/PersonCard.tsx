import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { Bot, GripVertical, MoreHorizontal } from "../../../../shared/ui/icons";
import type { PersonRow } from "../../model/unmapped";
import { Avatar } from "../Avatar";
import { FocusFirstItem, MenuButton, MenuItem, PeopleItems } from "./Menu";

const ICON_BUTTON =
  "grid size-6 shrink-0 place-items-center rounded-md text-content/50 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent";

export function AgentMark() {
  return (
    <span className="flex shrink-0 items-center gap-1 rounded-full bg-content/8 px-1.5 py-0.5 text-[10px] text-content/60">
      <Bot className="size-3" strokeWidth={1.75} />
      AI agent
    </span>
  );
}

/** Edits a display name; Enter or blur commits, Escape reverts. */
function NameInput({
  name,
  onCommit,
  focusNow,
  onFocused,
}: {
  name: string;
  onCommit: (name: string) => void;
  focusNow: boolean;
  onFocused: () => void;
}) {
  const [draft, setDraft] = useState(name);
  const input = useRef<HTMLInputElement>(null);
  const cancelled = useRef(false);

  // The saved name can change underneath (a merge, a reload); follow it.
  useEffect(() => setDraft(name), [name]);
  useEffect(() => {
    if (!focusNow) return;
    input.current?.focus();
    input.current?.select();
    onFocused();
  }, [focusNow, onFocused]);

  const commit = () => {
    if (cancelled.current) {
      cancelled.current = false;
      return;
    }
    const next = draft.trim();
    if (!next) setDraft(name);
    else if (next !== name) onCommit(next);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      event.currentTarget.blur();
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      cancelled.current = true;
      setDraft(name);
      event.currentTarget.blur();
    }
  };
  return (
    <input
      ref={input}
      value={draft}
      aria-label={`Display name of ${name}`}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={onKeyDown}
      className="h-7 min-w-0 flex-1 rounded-md border border-transparent bg-transparent px-1.5 text-[13px] font-medium text-content outline-none hover:border-stroke focus:border-accent/60"
    />
  );
}

export type PersonCardProps = {
  row: PersonRow;
  color: string;
  colorOf: (id: string) => string;
  /** Other people, for the Merge into and Move to menus. */
  others: PersonRow[];
  readOnly: boolean;
  focusName: boolean;
  onNameFocused: () => void;
  /** Key of an identity row of this card that should take focus (after a move). */
  focusIdentity: string | null;
  onIdentityFocused: () => void;
  onRename: (name: string) => void;
  onMergeInto: (target: PersonRow) => void;
  onRemove: () => void;
  /** From the Move to menu. */
  onMoveIdentity: (key: string, target: PersonRow) => void;
  /** Excludes one identity from the scores. Never offered for an agent. */
  onExcludeIdentity: (key: string) => void;
  /** Excludes every non-agent identity of the person. */
  onExcludePerson: () => void;
  /** An identity of this person is being dragged over this card. */
  dropActive: boolean;
  /** Key of the identity being dragged, to dim its row. */
  draggingKey: string | null;
  onIdentityPointerDown: (key: string, event: ReactPointerEvent) => void;
};

function PersonMenuBody({
  row,
  others,
  colorOf,
  close,
  onMergeInto,
  onRemove,
  onExcludePerson,
}: Pick<PersonCardProps, "row" | "others" | "colorOf"> & {
  close: () => void;
  onMergeInto: PersonCardProps["onMergeInto"];
  onRemove: PersonCardProps["onRemove"];
  onExcludePerson: PersonCardProps["onExcludePerson"];
}) {
  const excludable = row.identities.filter((i) => !i.agent).length;
  const [view, setView] = useState<"root" | "merge">("root");
  if (view === "merge") {
    return (
      <>
        <FocusFirstItem key="merge" />
        <div className="px-2.5 py-1 text-[11px] text-content/45">
          Merge {row.name} into
        </div>
        <PeopleItems
          people={others}
          colorOf={colorOf}
          onPick={(target) => {
            close();
            onMergeInto(target);
          }}
        />
      </>
    );
  }
  return (
    <>
      <MenuItem
        onSelect={() => setView("merge")}
        disabled={others.length === 0}
      >
        Merge into...
      </MenuItem>
      <MenuItem
        danger
        disabled={excludable === 0}
        description={
          excludable === 0
            ? "Agents cannot be excluded."
            : `Excludes all ${excludable} ${excludable === 1 ? "identity" : "identities"} from the scores.`
        }
        onSelect={() => {
          close();
          onExcludePerson();
        }}
      >
        Exclude person
      </MenuItem>
      <MenuItem
        danger
        disabled={!row.inConfig}
        onSelect={() => {
          close();
          onRemove();
        }}
      >
        Remove person
      </MenuItem>
    </>
  );
}

export function PersonCard(props: PersonCardProps) {
  const {
    row,
    color,
    colorOf,
    others,
    readOnly,
    focusName,
    onNameFocused,
    focusIdentity,
    onIdentityFocused,
    onRename,
    onMergeInto,
    onRemove,
    onMoveIdentity,
    onExcludeIdentity,
    onExcludePerson,
    dropActive,
    draggingKey,
    onIdentityPointerDown,
  } = props;
  const count = row.identities.length;
  const list = useRef<HTMLUListElement>(null);

  // A moved identity appears here only once the new config lands, so wait for
  // its row instead of focusing at the click.
  useEffect(() => {
    if (!focusIdentity) return;
    const target = Array.from(
      list.current?.querySelectorAll<HTMLElement>("[data-identity]") ?? [],
    ).find((el) => el.dataset.identity === focusIdentity);
    if (!target) return;
    target.focus();
    onIdentityFocused();
  }, [focusIdentity, row.identities, onIdentityFocused]);

  return (
    <div
      data-person={row.id}
      data-person-drop={readOnly ? undefined : row.id}
      className={`rounded-xl border px-3 py-2.5 ${
        dropActive
          ? "border-accent/60 bg-accent/10"
          : "border-stroke bg-content/3"
      }`}
    >
      <div className="flex items-center gap-2">
        <Avatar name={row.name} color={color} size="md" />
        {readOnly ? (
          <span className="min-w-0 flex-1 truncate px-1.5 text-[13px] font-medium text-content">
            {row.name}
          </span>
        ) : (
          <NameInput
            name={row.name}
            onCommit={onRename}
            focusNow={focusName}
            onFocused={onNameFocused}
          />
        )}
        <span className="shrink-0 text-[12px] tabular-nums text-content/50">
          {count} {count === 1 ? "identity" : "identities"} · {row.commits}{" "}
          {row.commits === 1 ? "commit" : "commits"}
        </span>
        {readOnly ? null : (
          <MenuButton
            label={`Actions for ${row.name}`}
            menuLabel={`Actions for ${row.name}`}
            triggerClassName={ICON_BUTTON}
            trigger={<MoreHorizontal className="size-4" strokeWidth={1.75} />}
          >
            {(close) => (
              <PersonMenuBody
                row={row}
                others={others}
                colorOf={colorOf}
                close={close}
                onMergeInto={onMergeInto}
                onRemove={onRemove}
                onExcludePerson={onExcludePerson}
              />
            )}
          </MenuButton>
        )}
      </div>

      <ul ref={list} className="mt-2 space-y-1">
        {row.identities.map((identity) => (
          <li
            key={identity.key}
            data-identity={identity.key}
            tabIndex={-1}
            onPointerDown={
              readOnly
                ? undefined
                : (event) => onIdentityPointerDown(identity.key, event)
            }
            className={`flex items-center gap-2 rounded-md bg-content/5 px-2 py-1 outline-none focus-visible:outline-2 focus-visible:outline-accent ${
              readOnly ? "" : "cursor-grab touch-none select-none"
            } ${draggingKey === identity.key ? "opacity-40" : ""}`}
          >
            {readOnly ? null : (
              <GripVertical
                aria-hidden
                className="size-3.5 shrink-0 text-content/35"
                strokeWidth={1.75}
              />
            )}
            <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-content/80">
              {identity.key}
            </span>
            {identity.agent ? <AgentMark /> : null}
            {readOnly ? null : (
              <MenuButton
                label={`Actions for identity ${identity.key}`}
                menuLabel={`Actions for identity ${identity.key}`}
                triggerClassName="grid size-5 shrink-0 place-items-center rounded-md text-content/50 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
                trigger={
                  <MoreHorizontal className="size-3.5" strokeWidth={1.75} />
                }
              >
                {(close) => (
                  <MenuItem
                    danger
                    disabled={identity.agent}
                    description={
                      identity.agent
                        ? "Agents cannot be excluded."
                        : "Leaves it out of the scores."
                    }
                    onSelect={() => {
                      close();
                      onExcludeIdentity(identity.key);
                    }}
                  >
                    Exclude identity
                  </MenuItem>
                )}
              </MenuButton>
            )}
            {readOnly ? null : (
              <MenuButton
                label={`Move ${identity.key} to another person`}
                menuLabel="Move to"
                triggerClassName="shrink-0 rounded-md px-1.5 py-0.5 text-[11px] text-content/50 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
                trigger="Move to..."
              >
                {(close) => (
                  <PeopleItems
                    people={others}
                    colorOf={colorOf}
                    onPick={(target) => {
                      close();
                      onMoveIdentity(identity.key, target);
                    }}
                  />
                )}
              </MenuButton>
            )}
          </li>
        ))}
      </ul>
      {readOnly ? null : (
        <div className="mt-1.5 rounded-md border border-dashed border-stroke px-2 py-1 text-center text-[11px] text-content/40">
          Drop an identity here
        </div>
      )}
    </div>
  );
}
