import { useState } from "react";
import { ChevronDown, ChevronRight } from "../../../../shared/ui/icons";
import { SecondaryButton } from "../../../../shared/ui/SecondaryButton";
import type {
  ExcludedIdentity,
  PersonRow,
  UnmappedIdentity,
} from "../../model/unmapped";
import { MenuButton, PeopleItems } from "./Menu";
import { AgentMark } from "./PersonCard";

export function UnmappedCard({
  unmapped,
  excluded,
  targets,
  locked = false,
  colorOf,
  onExclude,
  onInclude,
  onAddToPerson,
}: {
  unmapped: UnmappedIdentity[];
  excluded: ExcludedIdentity[];
  /** People an identity can be added to; null hides the menu (merging is off). */
  targets: PersonRow[] | null;
  /** The config file cannot be written, so the actions are disabled. */
  locked?: boolean;
  colorOf: (id: string) => string;
  onExclude: (key: string) => void;
  onInclude: (item: ExcludedIdentity) => void;
  onAddToPerson: (key: string, target: PersonRow) => void;
}) {
  const [showExcluded, setShowExcluded] = useState(false);
  return (
    <section data-unmapped-card className="rounded-xl border border-stroke">
      <h3 className="flex items-baseline gap-2 border-b border-stroke px-4 py-2.5 text-[13px] font-medium text-content">
        Unmapped identities
        <span className="text-[11px] font-normal tabular-nums text-content/45">
          {unmapped.length}
        </span>
      </h3>
      <p className="border-b border-stroke px-4 py-2 text-[11px] leading-relaxed text-content/50">
        Agents and machine accounts that no person lists yet. Add each to the
        person who owns it, or exclude it. Ordinary authors are not listed: each
        already counts as a person of their own, and can be excluded from the
        menu on their card.
      </p>
      {unmapped.length === 0 ? (
        <p className="px-4 py-5 text-center text-[12px] text-content/50">
          Nothing needs attention: no unowned agents or machine accounts.
        </p>
      ) : (
        <ul className="divide-y divide-stroke">
          {unmapped.map((item) => (
            <li
              key={item.key}
              data-unmapped={item.key}
              className="flex flex-wrap items-center gap-x-2 gap-y-1 px-4 py-2"
            >
              <span className="min-w-0 flex-1 basis-40">
                <span className="block truncate font-mono text-[12px] text-content/80">
                  {item.key}
                </span>
                <span className="text-[11px] tabular-nums text-content/45">
                  {item.commits} {item.commits === 1 ? "commit" : "commits"}
                  {item.reason === "machine" ? " · machine account" : ""}
                </span>
              </span>
              {item.agent ? (
                <>
                  <AgentMark />
                  <span className="basis-full text-[11px] text-content/50">
                    Agents are credited to the human (see How is the score
                    calculated?)
                  </span>
                </>
              ) : (
                <SecondaryButton
                  disabled={locked}
                  onClick={() => onExclude(item.key)}
                >
                  Exclude
                </SecondaryButton>
              )}
              {targets ? (
                <MenuButton
                  label={`Add ${item.key} to a person`}
                  disabled={locked}
                  menuLabel="Add to person"
                  triggerClassName="flex shrink-0 items-center gap-1.5 rounded-md border border-content/10 px-2.5 py-1 text-[12px] text-content/70 hover:bg-content/10 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
                  trigger="Add to person..."
                >
                  {(close) => (
                    <PeopleItems
                      people={targets.filter((p) => p.id !== item.key)}
                      colorOf={colorOf}
                      onPick={(target) => {
                        close();
                        onAddToPerson(item.key, target);
                      }}
                    />
                  )}
                </MenuButton>
              ) : null}
            </li>
          ))}
        </ul>
      )}
      {excluded.length > 0 ? (
        <div className="border-t border-stroke">
          <button
            type="button"
            aria-expanded={showExcluded}
            onClick={() => setShowExcluded((v) => !v)}
            className="flex w-full items-center gap-1.5 px-4 py-2 text-left text-[12px] text-content/60 hover:text-content focus-visible:outline-2 focus-visible:outline-accent"
          >
            {showExcluded ? (
              <ChevronDown className="size-3.5" strokeWidth={1.75} />
            ) : (
              <ChevronRight className="size-3.5" strokeWidth={1.75} />
            )}
            {`Excluded (${excluded.length})`}
          </button>
          {showExcluded ? (
            <ul className="divide-y divide-stroke border-t border-stroke">
              {excluded.map((item) => (
                <li
                  key={item.key}
                  data-excluded={item.key}
                  className="flex flex-wrap items-center gap-x-2 gap-y-1 px-4 py-2"
                >
                  <span className="min-w-0 flex-1 basis-40">
                    <span className="block truncate font-mono text-[12px] text-content/65">
                      {item.key}
                    </span>
                    {item.removable ? null : (
                      <span className="text-[11px] text-content/45">
                        {`Excluded by pattern ${item.patterns.join(", ")}. Edit excludeIdentities in .monoptah/contributors.json to change this.`}
                      </span>
                    )}
                  </span>
                  {item.removable ? (
                    <SecondaryButton
                      disabled={locked}
                      onClick={() => onInclude(item)}
                    >
                      Include again
                    </SecondaryButton>
                  ) : null}
                </li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
