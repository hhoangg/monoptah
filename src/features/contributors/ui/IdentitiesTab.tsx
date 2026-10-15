import { useCallback, useMemo, useState } from "react";
import { CircleAlert, Plus } from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import {
  createPerson,
  dismissMerge,
  excludeIdentity,
  includeIdentity,
  mergePeople,
  movePersonIdentity,
  personIdFor,
  renamePerson,
  suggestMerges,
} from "../model/identity";
import type { ContributorsModel } from "../model/pipeline";
import { colorFromMap } from "../model/palette";
import type { ContributorsConfig } from "../model/types";
import {
  ensureConfigPerson,
  excludedIdentities,
  personRows,
  removePerson,
} from "../model/unmapped";
import type { PersonRow } from "../model/unmapped";
import { DragGhost } from "./identities/DragGhost";
import { PersonCard } from "./identities/PersonCard";
import { SuggestionsCard } from "./identities/SuggestionsCard";
import type { Suggestion } from "./identities/SuggestionsCard";
import { UnmappedCard } from "./identities/UnmappedCard";
import { useIdentityDrag } from "./identities/useIdentityDrag";

export type IdentitiesTabProps = {
  model: ContributorsModel;
  config: ContributorsConfig;
  onChangeConfig: (next: ContributorsConfig) => void | Promise<void>;
  /** The config file cannot be read: every control that would write it is disabled. */
  locked?: boolean;
};

const NEW_PERSON_NAME = "New person";

export function IdentitiesTab({
  model,
  config,
  onChangeConfig,
  locked = false,
}: IdentitiesTabProps) {
  const [focusId, setFocusId] = useState<string | null>(null);
  // The identity row to focus once a move lands, and the person it moved to.
  const [focusRow, setFocusRow] = useState<{
    key: string;
    personId: string;
  } | null>(null);
  const merging = model.merge;
  const editable = merging && !locked;

  const rows = useMemo(
    () =>
      personRows({
        raw: model.raw,
        config: model.config,
        resolver: model.resolver,
      }),
    [model],
  );
  const colorOf = (id: string) => colorFromMap(model.colors, id);
  const excluded = useMemo(
    () => excludedIdentities({ raw: model.raw, config: model.config }),
    [model],
  );
  const suggestions = useMemo<Suggestion[]>(() => {
    // Raw identities are not what the config maps, so nothing is suggested.
    if (!model.merge) return [];
    const byId = new Map(rows.map((row) => [row.id, row]));
    const people = model.resolver.people.filter(
      (p) => byId.has(p.id) && p.identities.length > 0,
    );
    return suggestMerges(
      people,
      model.config.dismissedMerges,
      model.config,
    ).flatMap((suggestion) => {
      const involved = suggestion.personIds
        .map((id) => byId.get(id))
        .filter((row): row is PersonRow => !!row);
      if (involved.length < 2) return [];
      // The busiest person is kept; a config person wins a tie.
      involved.sort(
        (a, b) =>
          b.commits - a.commits ||
          Number(b.inConfig) - Number(a.inConfig) ||
          a.name.localeCompare(b.name),
      );
      return [{ suggestion, people: involved }];
    });
  }, [rows, model]);

  // A failed save is reported once, by the page; here it only means "not done".
  const apply = useCallback(
    async (next: ContributorsConfig) => {
      if (next === config || locked) return false;
      try {
        await onChangeConfig(next);
        return true;
      } catch {
        return false;
      }
    },
    [config, locked, onChangeConfig],
  );

  /** `focus` hands keyboard focus to the target's name, for the menu paths. */
  const moveIdentity = (key: string, target: PersonRow, focus = false) => {
    if (target.identities.some((i) => i.key === key)) return;
    // The source is listed too, so the rest of a mailmap group stays with it
    // instead of following the moved identity to the target.
    const source = rows.find((row) =>
      row.identities.some((i) => i.key === key),
    );
    let next = ensureConfigPerson(config, target);
    if (source) next = ensureConfigPerson(next, source);
    if (focus) setFocusRow({ key, personId: target.id });
    void apply(movePersonIdentity(next, key, target.id)).then((saved) => {
      if (!saved) setFocusRow(null);
    });
  };
  const mergeInto = (target: PersonRow, others: PersonRow[]) => {
    const base = others.reduce(
      (next, other) => ensureConfigPerson(next, other),
      ensureConfigPerson(config, target),
    );
    setFocusId(target.id);
    return apply(
      mergePeople(base, [target.id, ...others.map((p) => p.id)], target.name),
    );
  };
  const { drag, start } = useIdentityDrag({
    onDrop: (key, personId) => {
      const target = rows.find((row) => row.id === personId);
      if (target) moveIdentity(key, target);
    },
  });
  const addPerson = () => {
    setFocusId(personIdFor(config, NEW_PERSON_NAME));
    void apply(createPerson(config, NEW_PERSON_NAME));
  };
  const clearFocus = useCallback(() => setFocusId(null), []);
  const clearFocusRow = useCallback(() => setFocusRow(null), []);

  return (
    <div className="grid grid-cols-1 items-start gap-4 @3xl/contrib:grid-cols-2">
      <section className="rounded-xl border border-stroke">
        <h3 className="flex items-center gap-2 border-b border-stroke px-4 py-2.5 text-[13px] font-medium text-content">
          People
          <span className="text-[11px] font-normal text-content/45">
            rename a person, drag identities between people
          </span>
          <span className="flex-1" />
          <SecondaryButton onClick={addPerson} disabled={!editable}>
            <Plus className="size-3" strokeWidth={1.75} />
            New person
          </SecondaryButton>
        </h3>
        <div className="space-y-2.5 p-3">
          {merging ? null : (
            <div
              role="status"
              className="flex items-start gap-2 rounded-lg border border-amber-400/30 bg-amber-400/5 px-3 py-2 text-[12px] text-content/70"
            >
              <CircleAlert
                className="mt-0.5 size-3.5 shrink-0 text-amber-400"
                strokeWidth={1.75}
              />
              Merging is off: showing raw git identities. Turn on Merge
              identities in the header to apply this mapping.
            </div>
          )}
          {rows.length === 0 ? (
            <p className="py-4 text-center text-[12px] text-content/50">
              No people yet.
            </p>
          ) : null}
          {rows.map((row) => (
            <PersonCard
              key={row.id}
              row={row}
              color={colorOf(row.id)}
              colorOf={colorOf}
              others={rows.filter((p) => p.id !== row.id)}
              readOnly={!editable}
              focusName={focusId === row.id}
              onNameFocused={clearFocus}
              focusIdentity={
                focusRow?.personId === row.id ? focusRow.key : null
              }
              onIdentityFocused={clearFocusRow}
              onRename={(name) =>
                void apply(
                  renamePerson(ensureConfigPerson(config, row), row.id, name),
                )
              }
              onMergeInto={(target) => void mergeInto(target, [row])}
              onRemove={() => void apply(removePerson(config, row.id))}
              onMoveIdentity={(key, target) => moveIdentity(key, target, true)}
              onExcludeIdentity={(key) => {
                // Agents win over exclusion, so excluding one would do nothing.
                if (row.identities.some((i) => i.key === key && i.agent))
                  return;
                void apply(excludeIdentity(config, key));
              }}
              onExcludePerson={() =>
                void apply(
                  row.identities
                    .filter((i) => !i.agent)
                    .reduce((next, i) => excludeIdentity(next, i.key), config),
                )
              }
              dropActive={
                !!drag &&
                drag.overPersonId === row.id &&
                !row.identities.some((i) => i.key === drag.key)
              }
              draggingKey={drag?.key ?? null}
              onIdentityPointerDown={(key, event) => start(key, row.id, event)}
            />
          ))}
        </div>
        <p className="border-t border-stroke px-4 py-2.5 text-[11px] text-content/50">
          Saved to{" "}
          <code className="font-mono">.monoptah/contributors.json</code> in the
          repo (shared with the team).
        </p>
      </section>

      <div className="space-y-4">
        <SuggestionsCard
          suggestions={suggestions}
          colorOf={colorOf}
          readOnly={!merging}
          locked={locked}
          onMerge={(item) => {
            const [target, ...others] = item.people;
            void mergeInto(target, others);
          }}
          onDismiss={(item) =>
            void apply(dismissMerge(config, item.suggestion.id))
          }
        />
        <UnmappedCard
          unmapped={model.unmapped}
          excluded={excluded}
          targets={merging ? rows : null}
          locked={locked}
          colorOf={colorOf}
          onExclude={(key) => void apply(excludeIdentity(config, key))}
          onInclude={(item) =>
            void apply(
              item.patterns.reduce(
                (next, pattern) => includeIdentity(next, pattern),
                config,
              ),
            )
          }
          onAddToPerson={(key, target) => moveIdentity(key, target, true)}
        />
      </div>
      {drag ? <DragGhost drag={drag} /> : null}
    </div>
  );
}
