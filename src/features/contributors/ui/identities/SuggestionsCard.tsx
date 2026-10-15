import type { MergeSuggestion } from "../../model/identity";
import type { PersonRow } from "../../model/unmapped";
import { SecondaryButton } from "../../../../shared/ui/SecondaryButton";
import { Avatar } from "../Avatar";

const REASON: Record<MergeSuggestion["reason"], string> = {
  "same-name": "Same name",
  "similar-email": "Similar email",
  "github-noreply": "GitHub noreply",
};

export type Suggestion = {
  suggestion: MergeSuggestion;
  /** The people involved; the first one is kept when merging. */
  people: PersonRow[];
};

export function SuggestionsCard({
  suggestions,
  colorOf,
  onMerge,
  onDismiss,
  readOnly = false,
  locked = false,
}: {
  readOnly?: boolean;
  /** The config file cannot be written, so the actions are disabled. */
  locked?: boolean;
  suggestions: Suggestion[];
  colorOf: (id: string) => string;
  onMerge: (item: Suggestion) => void;
  onDismiss: (item: Suggestion) => void;
}) {
  return (
    <section className="rounded-xl border border-stroke">
      <h3 className="flex items-baseline gap-2 border-b border-stroke px-4 py-2.5 text-[13px] font-medium text-content">
        Suggested merges
        <span className="text-[11px] font-normal text-content/45">
          same name / similar email
        </span>
      </h3>
      {readOnly ? (
        <p role="status" className="px-4 py-5 text-[12px] text-content/60">
          Merging is off: showing raw git identities. Turn on Merge identities
          in the header to apply this mapping.
        </p>
      ) : suggestions.length === 0 ? (
        <p className="px-4 py-5 text-center text-[12px] text-content/50">
          No suggestions
        </p>
      ) : (
        <ul className="divide-y divide-stroke">
          {suggestions.map((item) => {
            const [keep] = item.people;
            return (
              <li
                key={item.suggestion.id}
                data-suggestion={item.suggestion.id}
                className="space-y-1.5 px-4 py-3"
              >
                <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px] text-content">
                  {item.people.map((p, index) => (
                    <span key={p.id} className="flex items-center gap-1.5">
                      {index > 0 ? (
                        <span aria-hidden className="text-content/40">
                          &harr;
                        </span>
                      ) : null}
                      <Avatar name={p.name} color={colorOf(p.id)} size="sm" />
                      <b className="font-medium">{p.name}</b>
                    </span>
                  ))}
                </div>
                <div className="text-[11px] text-content/50">
                  {REASON[item.suggestion.reason]}
                </div>
                <div className="break-all font-mono text-[11px] text-content/65">
                  {item.people
                    .flatMap((p) => p.identities.map((i) => i.key))
                    .join(" · ")}
                </div>
                <div className="flex gap-2 pt-0.5">
                  <SecondaryButton
                    disabled={locked}
                    onClick={() => onMerge(item)}
                  >
                    {`Merge into ${keep.name}`}
                  </SecondaryButton>
                  <SecondaryButton
                    disabled={locked}
                    onClick={() => onDismiss(item)}
                  >
                    Dismiss
                  </SecondaryButton>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
