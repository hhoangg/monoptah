import { useRef, type KeyboardEvent } from "react";
import { haptic } from "@/lib/haptics";
import { useAppearance } from "@/lib/appearance";
import { cn } from "@/lib/utils";
import { ACCENTS, THEME_LABELS, THEMES, type Accent, type Scheme, type Theme } from "@/styles/themes";
import { Icon } from "@/ui/Icon";
import { SettingsCard } from "@/ui/SettingsCard";
import { ACCENT_LABELS, accentSwatch, themeSwatch } from "./swatches";

interface Choice<T extends string> {
  value: T;
  label: string;
  /** Fills the swatch circle; `dot` (optional) is the inner dot. */
  back: string;
  dot?: string;
}

interface SwatchGroupProps<T extends string> {
  label: string;
  choices: ReadonlyArray<Choice<T>>;
  value: T;
  onChange: (value: T) => void;
}

/** A radiogroup of colour swatches: roving tabindex, arrows/Home/End move and select, selection is a ring plus a check mark. */
function SwatchGroup<T extends string>({ label, choices, value, onChange }: SwatchGroupProps<T>) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);

  function select(index: number) {
    const choice = choices[index];
    if (!choice) return;
    if (choice.value !== value) haptic("selection");
    onChange(choice.value);
    refs.current[index]?.focus();
  }

  function onKeyDown(event: KeyboardEvent, index: number) {
    const last = choices.length - 1;
    const next =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? (index + 1) % choices.length
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? (index + last) % choices.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : null;
    if (next === null) return;
    event.preventDefault();
    select(next);
  }

  const checkedIndex = Math.max(
    0,
    choices.findIndex((choice) => choice.value === value),
  );

  return (
    <div role="radiogroup" aria-label={label} className="flex flex-wrap gap-x-1 gap-y-2 px-1 py-2">
      {choices.map((choice, index) => {
        const checked = choice.value === value;
        return (
          <button
            key={choice.value}
            ref={(node) => {
              refs.current[index] = node;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={index === checkedIndex ? 0 : -1}
            onClick={() => select(index)}
            onKeyDown={(event) => onKeyDown(event, index)}
            className="flex min-h-11 w-20 flex-col items-center gap-1 rounded-row px-1 py-1.5 text-meta text-content hover:bg-selection"
          >
            <span
              aria-hidden
              style={{ backgroundColor: choice.back }}
              className={cn(
                "relative inline-flex size-10 items-center justify-center rounded-pill border border-stroke",
                checked && "outline-2 outline-offset-2 outline-content",
              )}
            >
              {choice.dot ? <span style={{ backgroundColor: choice.dot }} className="size-4 rounded-pill" /> : null}
              {checked ? (
                <span className="absolute -right-1 -bottom-1 inline-flex size-4 items-center justify-center rounded-pill bg-content text-background">
                  <Icon name="check" size={11} />
                </span>
              ) : null}
            </span>
            <span className={cn("text-center", checked ? "font-medium" : "text-muted-text")}>{choice.label}</span>
          </button>
        );
      })}
    </div>
  );
}

/**
 * Theme and accent pickers (F-88). Six themes and seven accents plus "Theme default"; a choice applies at
 * once through `useAppearance`, which sets `data-theme` / `data-accent` and stores it on this device.
 */
export function ThemeSection() {
  const theme = useAppearance((state) => state.theme);
  const accent = useAppearance((state) => state.accent);
  const scheme: Scheme = useAppearance((state) => state.scheme);
  const setTheme = useAppearance((state) => state.setTheme);
  const setAccent = useAppearance((state) => state.setAccent);

  const themeChoices: Array<Choice<Theme>> = THEMES.map((value) => {
    const swatch = themeSwatch(value, scheme);
    return { value, label: THEME_LABELS[value], back: swatch.background, dot: swatch.accent };
  });
  const accentChoices: Array<Choice<Accent>> = ACCENTS.map((value) => ({
    value,
    label: ACCENT_LABELS[value],
    back: accentSwatch(value, theme, scheme),
  }));

  return (
    <SettingsCard title="Theme" footnote="Theme tints the surfaces. An accent replaces the theme's own accent colour. Saved on this device only.">
      <SwatchGroup label="Theme" choices={themeChoices} value={theme} onChange={setTheme} />
      <div role="separator" className="mx-1.5 h-px bg-stroke" />
      <SwatchGroup label="Accent colour" choices={accentChoices} value={accent} onChange={setAccent} />
    </SettingsCard>
  );
}
