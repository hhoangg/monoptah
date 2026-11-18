// F-75: create a Mono, made in one go as a messaging app makes a contact: its look picked here (name, one of 10
// mascots, one of 9 colours), then Create runs `mono.create` and `mono.update` with that look and opens the
// chat. Control devices only and only when the desktop advertises `monoTools`: the caller (MonoHome) does not
// mount it otherwise.
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useRef, useState } from "react";
import { catalogKey, useLinkMutation } from "@/data";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { Notice } from "@/ui/Notice";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { MONO_MASCOT_NAMES, MONO_NAME_MAX, MONO_SWATCHES, MONO_SWATCH_NAMES, randomLook } from "./monoLook";
import { MonoMascot } from "./MonoMascot";
import { RadioGroup, radioTabIndex } from "./RadioGroup";

export function CreateMonoDialog({ desktopId, open, onOpenChange }: { desktopId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  // A bottom sheet below 640px, a dialog from 640px.
  return (
    <ResponsiveOverlay open={open} onOpenChange={onOpenChange} title="New Mono">
      {/* Mounted only while open, so every opening starts from a fresh look. */}
      {open ? <Form desktopId={desktopId} onClose={() => onOpenChange(false)} /> : null}
    </ResponsiveOverlay>
  );
}

function Form({ desktopId, onClose }: { desktopId: string; onClose: () => void }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const create = useLinkMutation("mono.create");
  const update = useLinkMutation("mono.update");
  const [name, setName] = useState("");
  const [look, setLook] = useState(() => randomLook());
  const [error, setError] = useState<string | null>(null);
  // A create that went through but whose update did not: Create then retries only the update.
  const created = useRef<string | null>(null);
  const busy = create.isPending || update.isPending;
  const ready = name.trim().length > 0 && !busy;

  const submit = async () => {
    if (!ready) return;
    setError(null);
    try {
      const monoId = created.current ?? (await create.mutateAsync({})).monoId;
      created.current = monoId;
      await update.mutateAsync({ monoId, name: name.trim(), mascot: look.mascot, color: look.color });
      await queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) });
      onClose();
      void navigate({ to: "/d/$desktopId/monos/$monoId", params: { desktopId, monoId } });
    } catch (failure) {
      void queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) });
      setError(failure instanceof Error && failure.message ? failure.message : "Could not create a Mono.");
    }
  };

  return (
    <form
      className="flex flex-col gap-4"
      data-testid="create-mono"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <div className="flex justify-center">
        <MonoMascot monoId="new" mascot={look.mascot} color={look.color} size={96} />
      </div>
      <Input aria-label="Name" value={name} maxLength={MONO_NAME_MAX} placeholder="Name your Mono" autoFocus disabled={busy} onChange={(event) => setName(event.target.value)} />
      <RadioGroup aria-label="Mascot" className="grid grid-cols-5 gap-2">
        {MONO_MASCOT_NAMES.map((option, index) => (
          <button
            key={option}
            type="button"
            role="radio"
            aria-checked={option === look.mascot}
            aria-label={option}
            tabIndex={radioTabIndex(option === look.mascot, index, MONO_MASCOT_NAMES.includes(look.mascot))}
            disabled={busy}
            onClick={() => setLook({ ...look, mascot: option })}
            className={cn("grid aspect-square place-items-center rounded-card border-2 border-transparent hover:bg-selection", option === look.mascot && "border-[var(--accent-color)]")}
          >
            <MonoMascot monoId="new" mascot={option} color={look.color} size={36} />
          </button>
        ))}
      </RadioGroup>
      <RadioGroup aria-label="Color" className="flex flex-wrap justify-center gap-2">
        {MONO_SWATCHES.map((swatch, index) => {
          const selected = swatch === look.color;
          return (
            <button
              key={swatch}
              type="button"
              role="radio"
              aria-checked={selected}
              aria-label={MONO_SWATCH_NAMES[swatch]}
              title={MONO_SWATCH_NAMES[swatch]}
              tabIndex={radioTabIndex(selected, index, (MONO_SWATCHES as readonly string[]).includes(look.color))}
              disabled={busy}
              onClick={() => setLook({ ...look, color: swatch })}
              className="grid size-11 place-items-center rounded-pill border-2 border-transparent pointer-fine:size-9"
              style={selected ? { borderColor: swatch } : undefined}
            >
              <span className="grid size-7 place-items-center rounded-pill text-inverse" style={{ background: swatch }}>
                {selected ? <Icon name="check" size={14} /> : null}
              </span>
            </button>
          );
        })}
      </RadioGroup>
      {error ? <Notice variant="error">{error}</Notice> : null}
      <div className="flex flex-col-reverse gap-2 min-[40rem]:flex-row min-[40rem]:justify-end">
        <Button type="button" variant="ghost" disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <Button type="submit" disabled={!ready} aria-busy={busy}>
          Create
        </Button>
      </div>
    </form>
  );
}
