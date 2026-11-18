// F-23: the round filter button beside the project's search (ported from
// mobile/src/features/sessions/SessionFilterMenu.tsx): show every session or only those working now, and
// which agents to show. Lit while a filter is on.
import type { Harness } from "@monoptah/protocol";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { HarnessIcon } from "@/ui/HarnessIcon";
import { Icon } from "@/ui/Icon";
import { HARNESS_NAMES, isFilterOn, type SessionListFilter } from "./sessionListFilter";

export interface SessionFilterMenuProps {
  value: SessionListFilter;
  onChange: (patch: Partial<SessionListFilter>) => void;
  /** The agents the list has sessions from, in the canonical order. */
  harnesses: Harness[];
}

export function SessionFilterMenu({ value, onChange, harnesses }: SessionFilterMenuProps) {
  const on = isFilterOn(value);
  // A hidden agent stays listed even when none of the loaded rows is its, so it can be shown again.
  const agents = harnesses.length > 0 || value.hiddenHarnesses.length > 0 ? [...new Set([...harnesses, ...value.hiddenHarnesses])] : [];
  const toggle = (harness: Harness) =>
    onChange({
      hiddenHarnesses: value.hiddenHarnesses.includes(harness) ? value.hiddenHarnesses.filter((hidden) => hidden !== harness) : [...value.hiddenHarnesses, harness],
    });
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={on ? "Filter sessions, on" : "Filter sessions"}
          data-on={on}
          className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection data-[on=true]:text-brand pointer-fine:size-9"
        >
          <Icon name="filter" size={20} />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem role="menuitemradio" aria-checked={!value.working} onSelect={() => onChange({ working: false })}>
          <Icon name="inbox" size={18} />
          <span className="flex-1">All sessions</span>
          {!value.working ? <Icon name="check" size={16} /> : null}
        </DropdownMenuItem>
        <DropdownMenuItem role="menuitemradio" aria-checked={value.working} onSelect={() => onChange({ working: true })}>
          <Icon name="automation" size={18} />
          <span className="flex-1">Working now</span>
          {value.working ? <Icon name="check" size={16} /> : null}
        </DropdownMenuItem>
        {agents.length > 0 ? (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuLabel>Agents</DropdownMenuLabel>
            {agents.map((harness) => {
              const shown = !value.hiddenHarnesses.includes(harness);
              return (
                <DropdownMenuItem
                  key={harness}
                  role="menuitemcheckbox"
                  aria-checked={shown}
                  onSelect={(event) => {
                    event.preventDefault();
                    toggle(harness);
                  }}
                >
                  <HarnessIcon harness={harness} className="size-[18px]" />
                  <span className="flex-1">{HARNESS_NAMES[harness]}</span>
                  {shown ? <Icon name="check" size={16} /> : null}
                </DropdownMenuItem>
              );
            })}
          </>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
