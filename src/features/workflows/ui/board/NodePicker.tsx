import { Popover, type PopoverAnchor } from "../../../../shared/ui/Popover";
import type { NodeType } from "../../model/workflow";
import { NODE_ICON, NODE_LABEL } from "./boardNodes";

const GROUPS: { title: string; items: { type: NodeType; hint: string }[] }[] = [
  {
    title: "Trigger",
    items: [
      { type: "schedule", hint: "Runs on a schedule" },
      { type: "manual", hint: "Runs when you start it" },
    ],
  },
  {
    title: "Action",
    items: [
      { type: "session", hint: "A session works on a project" },
      { type: "command", hint: "A shell command" },
      { type: "askMono", hint: "A question for the Mono" },
    ],
  },
  {
    title: "Flow",
    items: [
      { type: "if", hint: "Go one way or the other" },
      { type: "forEach", hint: "Repeat for each item of a list" },
    ],
  },
  {
    title: "End",
    items: [{ type: "report", hint: "Tells the Mono how it went" }],
  },
];

/** Why a type cannot be added now, if it cannot. */
function unavailable(
  type: NodeType,
  has: { trigger: boolean; report: boolean },
): string | undefined {
  if ((type === "schedule" || type === "manual") && has.trigger)
    return "A workflow has one trigger";
  if (type === "report" && has.report) return "A workflow has one Report node";
  return undefined;
}

/** The menu behind + Node: every kind of node, grouped by its part in the workflow. */
export function NodePicker({
  anchor,
  has,
  onPick,
  onClose,
}: {
  anchor: PopoverAnchor;
  has: { trigger: boolean; report: boolean };
  onPick: (type: NodeType) => void;
  onClose: () => void;
}) {
  return (
    <Popover
      anchor={anchor}
      align="end"
      width={248}
      maxHeight={520}
      onDismiss={onClose}
      role="menu"
      aria-label="Add node"
      className="overflow-y-auto overscroll-none p-1"
    >
      {GROUPS.map((group) => (
        <div key={group.title} role="group" aria-label={group.title}>
          <div className="px-2 pb-0.5 pt-2 text-[10px] font-semibold uppercase tracking-[0.08em] text-content/40">
            {group.title}
          </div>
          {group.items.map(({ type, hint }) => {
            const Icon = NODE_ICON[type];
            const why = unavailable(type, has);
            return (
              <button
                key={type}
                type="button"
                role="menuitem"
                disabled={!!why}
                title={why}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => onPick(type)}
                className="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left enabled:hover:bg-content/5 disabled:opacity-40"
              >
                <span
                  aria-hidden
                  className="grid size-6 shrink-0 place-items-center rounded-md bg-content/6 text-content/60"
                >
                  <Icon className="size-3.5" strokeWidth={1.75} />
                </span>
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-[12px] leading-4 text-content/85">
                    {NODE_LABEL[type]}
                  </span>
                  <span className="truncate text-[11px] leading-4 text-content/40">
                    {why ?? hint}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      ))}
    </Popover>
  );
}
