// Copied from src/features/sessions/ui/AccessPicker.tsx at ca73ac2b
// The labels, hints and icons come from there; the popover became the shadcn DropdownMenu and the
// `full-access` choice is gone (it is shown read only when the desktop runs a session that way).
import { DropdownMenu as DropdownMenuPrimitive } from "radix-ui";
import type { RuntimeMode, SendableRuntimeMode } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import { MENU_ITEM, MENU_ITEM_ATTR, MENU_ITEM_HIGHLIGHTED } from "@/ui/menuSurface";
import { DropdownMenu, DropdownMenuContent, DropdownMenuLabel, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { Check, ChevronDown, Lock, Pencil, Shield, Sparkles } from "@/ui/icons";
import { ACCESS_MODES, FULL_ACCESS_DISPLAY, accessMode, isFullAccess, type AccessMode } from "./access";

const ICONS = { lock: Lock, edit: Pencil, sparkle: Sparkles } as const;

export interface AccessPickerProps {
  /** What the session runs with, as the desktop reports it (may be `full-access`). */
  value: RuntimeMode;
  /** Only ever called with a sendable mode. */
  onChange: (mode: SendableRuntimeMode) => void;
  onClose?: () => void;
  /** A turn is running: the change applies to the next turn. */
  busy?: boolean;
  /** A read-only device cannot change access. */
  disabled?: boolean;
  side?: "top" | "bottom";
  variant?: "pill" | "plain";
}

export function AccessPicker({
  value,
  onChange,
  onClose,
  busy = false,
  disabled = false,
  side = "top",
  variant = "pill",
}: AccessPickerProps) {
  const fullAccess = isFullAccess(value);
  const current: AccessMode | null = fullAccess ? null : accessMode(value);
  const label = fullAccess ? FULL_ACCESS_DISPLAY.label : current!.label;
  const hint = fullAccess ? FULL_ACCESS_DISPLAY.hint : current!.hint;
  const TriggerIcon = fullAccess ? Shield : ICONS[current!.icon];

  return (
    <DropdownMenu onOpenChange={(open) => !open && onClose?.()}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          data-access-picker-trigger
          title={`${hint}${busy ? " Changes apply to the next turn." : ""}`}
          aria-label={`Access: ${label}`}
          className={cn(
            "flex max-w-52 min-h-11 min-w-0 shrink-[4] items-center gap-1.5 text-caption pointer-fine:min-h-9 text-content disabled:opacity-40 data-[state=open]:bg-selection",
            variant === "plain" ? "rounded-row px-2 hover:bg-selection" : "rounded-pill bg-selection px-3",
          )}
        >
          <TriggerIcon size={14} className="shrink-0" />
          {/* Icon-only on a narrow toolbar: the model name keeps the room; the label stays in aria-label. */}
          <span className="min-w-0 truncate max-[400px]:hidden">{label}</span>
          <ChevronDown size={12} className="shrink-0 text-muted-text" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side={side} align="start" className="w-72" aria-label="Access">
        {fullAccess ? (
          <>
            <DropdownMenuLabel className="flex items-center gap-2 text-content">
              <Shield size={14} className="shrink-0" />
              {FULL_ACCESS_DISPLAY.note}
            </DropdownMenuLabel>
            <p className="px-3 pb-1.5 text-caption text-muted-text">{FULL_ACCESS_DISPLAY.hint}</p>
          </>
        ) : null}
        <DropdownMenuPrimitive.RadioGroup
          value={fullAccess ? "" : value}
          onValueChange={(next) => {
            const mode = ACCESS_MODES.find((item) => item.id === next);
            if (mode) onChange(mode.id);
          }}
        >
          {ACCESS_MODES.map((mode) => {
            const ModeIcon = ICONS[mode.icon];
            return (
              <DropdownMenuPrimitive.RadioItem
                key={mode.id}
                value={mode.id}
                {...MENU_ITEM_ATTR}
                className={`flex min-h-11 cursor-default items-start gap-2.5 ${MENU_ITEM} px-3 py-2 outline-none select-none ${MENU_ITEM_HIGHLIGHTED} pointer-fine:min-h-9`}
              >
                <ModeIcon size={14} className="mt-1 shrink-0 text-muted-text" />
                <span className="min-w-0 flex-1">
                  <span className="block text-row font-medium text-content">{mode.label}</span>
                  <span className="mt-0.5 block text-caption text-muted-text">{mode.hint}</span>
                </span>
                <DropdownMenuPrimitive.ItemIndicator className="mt-1 shrink-0">
                  <Check size={14} />
                </DropdownMenuPrimitive.ItemIndicator>
              </DropdownMenuPrimitive.RadioItem>
            );
          })}
        </DropdownMenuPrimitive.RadioGroup>
        {busy ? (
          <p className="px-3 py-1.5 text-caption text-muted-text">
            Access changes apply to the next turn. Stop and resend to apply them now.
          </p>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
