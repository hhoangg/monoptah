// Ported from EffortControl in mobile/src/features/models/ModelPicker.tsx: a stepped slider over the
// model's effort options, the label of the chosen step on the right, steps past "high" in caution.
import { Slider as SliderPrimitive } from "radix-ui";
import { cn } from "@/lib/utils";
import { HOT_EFFORT } from "./modelSettings";

export interface EffortSliderProps {
  label: string;
  options: ReadonlyArray<{ value: string; label: string }>;
  value: string;
  onChange: (value: string) => void;
  disabled?: boolean;
}

export function EffortSlider({ label, options, value, onChange, disabled = false }: EffortSliderProps) {
  const index = Math.max(0, options.findIndex((option) => option.value === value));
  const current = options[index];
  const hot = HOT_EFFORT.test(current?.value ?? "");
  const last = options.length - 1;
  return (
    <div className="flex flex-col gap-2" data-effort-slider>
      <div className="flex items-baseline justify-between gap-3">
        <span className="text-row font-medium text-content">
          {label}
        </span>
        <span className={cn("text-label", hot ? "text-caution" : "text-muted-text")} data-hot={hot || undefined}>
          {current?.label}
        </span>
      </div>
      <SliderPrimitive.Root
        value={[index]}
        min={0}
        max={Math.max(1, last)}
        step={1}
        disabled={disabled || options.length < 2}
        onValueChange={([next]) => {
          const option = options[Math.round(next ?? 0)];
          if (option && option.value !== value) onChange(option.value);
        }}
        className="relative flex h-8 w-full touch-none items-center select-none data-[disabled]:opacity-40"
      >
        <SliderPrimitive.Track className="relative h-1.5 w-full grow rounded-pill bg-fill">
          <SliderPrimitive.Range className={cn("absolute h-full rounded-pill", hot ? "bg-caution" : "bg-brand")} />
          {options.map((option, step) => (
            <span
              key={option.value}
              aria-hidden
              data-step
              className="absolute top-1/2 size-1 -translate-x-1/2 -translate-y-1/2 rounded-pill bg-stroke"
              style={{ left: `${last > 0 ? (step / last) * 100 : 0}%` }}
            />
          ))}
        </SliderPrimitive.Track>
        <SliderPrimitive.Thumb
          aria-label={label}
          aria-valuetext={current?.label}
          className={cn("block size-6 rounded-pill border border-stroke bg-white outline-none", hot && "border-caution")}
        />
      </SliderPrimitive.Root>
    </div>
  );
}
