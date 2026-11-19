import { motion, useReducedMotion } from "motion/react";
import { useId, useRef, type KeyboardEvent } from "react";
import { haptic } from "@/lib/haptics";
import { cn } from "@/lib/utils";
import { Icon, type IconName } from "./Icon";

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  icon?: IconName;
}

export interface SegmentedControlProps<T extends string> {
  /** Accessible name of the group. */
  label: string;
  options: ReadonlyArray<SegmentedOption<T>>;
  value: T;
  onChange: (value: T) => void;
  className?: string;
}

/**
 * Segmented switch (design-system.md section 8): track 44px high (52 on touch, so options stay 44) with a 4px inset, thumb radius 18
 * that slides in 240ms. It is a single choice, so it is a `radiogroup` with a roving tabindex and
 * Left/Right/Home/End keys, not a tablist (there are no panels to switch).
 */
export function SegmentedControl<T extends string>({ label, options, value, onChange, className }: SegmentedControlProps<T>) {
  const group = useId();
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const reduceMotion = useReducedMotion();

  function select(index: number) {
    const option = options[index];
    if (!option) return;
    haptic("selection");
    onChange(option.value);
    refs.current[index]?.focus();
  }

  function onKeyDown(event: KeyboardEvent, index: number) {
    const last = options.length - 1;
    const next =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? (index + 1) % options.length
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? (index + last) % options.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : null;
    if (next === null) return;
    event.preventDefault();
    select(next);
  }

  return (
    <div role="radiogroup" aria-label={label} className={cn("flex min-h-13 w-full gap-1 rounded-pill bg-fill p-1 pointer-fine:min-h-11 max-[19.9rem]:flex-col max-[19.9rem]:rounded-card", className)}>
      {options.map((option, index) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            ref={(node) => {
              refs.current[index] = node;
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            onClick={() => select(index)}
            onKeyDown={(event) => onKeyDown(event, index)}
            className="relative isolate inline-flex min-h-11 min-w-0 flex-1 pointer-fine:min-h-9 items-center justify-center gap-1.5 rounded-[18px] px-2 min-[24rem]:px-3 text-small font-semibold text-content outline-offset-2"
          >
            {selected ? (
              <motion.span
                layoutId={`${group}-thumb`}
                aria-hidden="true"
                className="absolute inset-0 -z-10 rounded-[18px] bg-thumb"
                transition={reduceMotion ? { duration: 0 } : { duration: 0.24, ease: [0.215, 0.61, 0.355, 1] }}
              />
            ) : null}
            {option.icon ? <Icon name={option.icon} size={18} className={cn("hidden min-[24rem]:inline-flex", selected ? undefined : "opacity-50")} /> : null}
            <span className={cn("min-w-0 truncate", selected ? undefined : "text-content-a72")}>{option.label}</span>
          </button>
        );
      })}
    </div>
  );
}
