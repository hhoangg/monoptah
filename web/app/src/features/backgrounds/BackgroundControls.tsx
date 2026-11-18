import { useId, useRef, useState } from "react";
import { Slider as SliderPrimitive } from "radix-ui";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { OPACITY_RANGE } from "./dither";
import { useBackgroundActions, useStoredBackground } from "./useBackground";

export interface BackgroundControlsProps {
  projectId: string;
  projectName: string;
}

/** Photo picker, opacity and remove for one project's empty-session background (F-90). Local to this device. */
export function BackgroundControls({ projectId, projectName }: BackgroundControlsProps) {
  const loaded = useStoredBackground(projectId);
  const record = loaded?.record ?? null;
  const preview = loaded?.url ?? null;
  const { busy, error, pick, setOpacity, remove } = useBackgroundActions(projectId, projectName, record);
  const input = useRef<HTMLInputElement>(null);
  const labelId = useId();
  const [draft, setDraft] = useState<number | null>(null);
  const percent = Math.round((draft ?? record?.opacity ?? 0) * 100);

  return (
    <div className="flex flex-col gap-3 px-1 py-2">
      <div className="flex items-center gap-3">
        {preview ? (
          <img src={preview} alt="" className="size-12 shrink-0 rounded-row bg-fill object-cover" />
        ) : (
          <span aria-hidden className="inline-flex size-12 shrink-0 items-center justify-center rounded-row bg-fill text-muted-text">
            <Icon name="image" size={20} />
          </span>
        )}
        <span className="min-w-0 flex-1 text-row text-content wrap-anywhere">{projectName}</span>
        <input
          ref={input}
          type="file"
          accept="image/*"
          className="sr-only"
          tabIndex={-1}
          aria-label={`Background photo for ${projectName}`}
          onChange={(event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            if (file) void pick(file);
          }}
        />
        <Button type="button" variant="secondary" size="sm" disabled={busy} onClick={() => input.current?.click()}>
          {record ? "Replace" : "Choose photo"}
        </Button>
        {record ? (
          <Button type="button" variant="ghost" size="sm" aria-label={`Remove background for ${projectName}`} disabled={busy} onClick={() => void remove()}>
            <Icon name="trash" size={18} />
          </Button>
        ) : null}
      </div>
      {busy ? <DelayedSpinner /> : null}
      {error ? (
        <p role="alert" className="text-meta text-deletions">
          {error}
        </p>
      ) : null}
      {record ? (
        <div className="flex flex-col gap-1">
          <div className="flex items-baseline justify-between gap-3">
            <span id={labelId} className="text-label text-muted-text">
              Opacity
            </span>
            <span className="text-label text-muted-text">{percent}%</span>
          </div>
          <SliderPrimitive.Root
            aria-labelledby={labelId}
            value={[draft ?? record.opacity]}
            min={OPACITY_RANGE.min}
            max={OPACITY_RANGE.max}
            step={0.01}
            onValueChange={([value]) => setDraft(value ?? null)}
            onValueCommit={([value]) => {
              setDraft(null);
              if (value !== undefined) void setOpacity(value);
            }}
            className="relative flex h-8 w-full touch-none items-center select-none"
          >
            <SliderPrimitive.Track className="relative h-1.5 w-full grow rounded-pill bg-fill">
              <SliderPrimitive.Range className="absolute h-full rounded-pill bg-brand" />
            </SliderPrimitive.Track>
            <SliderPrimitive.Thumb aria-label={`Opacity for ${projectName}`} className="block size-5 rounded-pill border border-stroke bg-background outline-offset-2" />
          </SliderPrimitive.Root>
        </div>
      ) : null}
    </div>
  );
}
