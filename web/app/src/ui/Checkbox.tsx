import { useId, type ComponentProps, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Icon } from "./Icon";

export interface CheckboxProps extends Omit<ComponentProps<"input">, "type" | "id" | "children"> {
  label: ReactNode;
  id?: string;
}

/** Native checkbox (design-system.md 3.3): 18x18, 1.5px border faint, radius 5, active fill content. Row is >= 44px. */
export function Checkbox({ label, id, className, ...input }: CheckboxProps) {
  const generated = useId();
  const inputId = id ?? generated;
  return (
    <label htmlFor={inputId} className={cn("flex min-h-11 cursor-pointer items-center gap-3 text-body text-content", className)}>
      <span className="relative inline-flex size-[18px] shrink-0">
        <input
          id={inputId}
          type="checkbox"
          className="peer size-[18px] cursor-pointer appearance-none rounded-[5px] border-[1.5px] border-faint bg-transparent checked:border-content checked:bg-content"
          {...input}
        />
        <Icon name="check" size={14} strokeWidth={2.5} className="pointer-events-none absolute top-0.5 left-0.5 hidden text-inverse peer-checked:block" />
      </span>
      <span className="min-w-0 flex-1">{label}</span>
    </label>
  );
}
