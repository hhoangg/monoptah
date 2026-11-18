// Copied from src/features/sessions/ui/HarnessIcon.tsx at ca73ac2b
// Provider logos render as <img> or as a CSS mask over the SVG file (Vite URL imports). No markup
// is ever injected. Trademark note: web/app/src/assets/providers/NOTICE.
import type { CSSProperties } from "react";
import type { Harness } from "@monoptah/protocol";
import { cn } from "@/lib/utils";
import antigravity from "@/assets/providers/antigravity.svg";
import claude from "@/assets/providers/claude.svg";
import providerNotice from "@/assets/providers/NOTICE?raw";
import codex from "@/assets/providers/codex.svg";
import cursor from "@/assets/providers/cursor.svg";
import devin from "@/assets/providers/devin.svg";
import fx from "@/assets/providers/fx.svg";
import grok from "@/assets/providers/grok.svg";
import hermes from "@/assets/providers/hermes.svg";
import omp from "@/assets/providers/omp.svg";
import opencode from "@/assets/providers/opencode.svg";
import pi from "@/assets/providers/pi.svg";

/** The trademark note for the provider marks. It is rendered (see `ProviderNotice`), so it ships with the built app. */
export const PROVIDER_NOTICE: string = providerNotice.trim();

/** The provider trademark note as small print, shown where the marks are listed (the model picker). */
export function ProviderNotice({ className }: { className?: string }) {
  return (
    <p data-testid="provider-notice" className={cn("text-meta text-muted-text", className)}>
      {PROVIDER_NOTICE}
    </p>
  );
}

export const HARNESS_ICONS: Record<Harness, string> = {
  claude,
  codex,
  cursor,
  grok,
  opencode,
  pi,
  omp,
  fx,
  hermes,
  antigravity,
  devin,
};

/** Single-colour marks that follow `currentColor` so they stay visible in light mode. */
export const MONOCHROME_HARNESSES: ReadonlySet<string> = new Set<Harness>([
  "cursor",
  "grok",
  "opencode",
  "pi",
  "fx",
  "hermes",
  "devin",
]);

/** Share of the box the mark fills when masked (hermes and devin carry their own padding). */
const MASK_SCALE: Partial<Record<Harness, string>> = { hermes: "72%", devin: "88%" };

export function isHarness(value: string): value is Harness {
  return Object.hasOwn(HARNESS_ICONS, value);
}

export interface HarnessIconProps {
  /** A harness id from the desktop. An unknown id renders nothing (the id is open on the wire). */
  harness: string;
  className?: string;
}

export function HarnessIcon({ harness, className = "size-3.5" }: HarnessIconProps) {
  if (!isHarness(harness)) return null;
  const mark = HARNESS_ICONS[harness];
  if (MONOCHROME_HARNESSES.has(harness)) {
    const size = MASK_SCALE[harness] ?? "100%";
    // The URL comes from a build-time import, not from the desktop. Quoted: Vite inlines small
    // SVGs as data URIs that contain spaces.
    const mask = `url("${mark}")`;
    const style: CSSProperties = {
      width: size,
      height: size,
      backgroundColor: "currentColor",
      maskImage: mask,
      maskPosition: "center",
      maskRepeat: "no-repeat",
      maskSize: "contain",
      WebkitMaskImage: mask,
      WebkitMaskPosition: "center",
      WebkitMaskRepeat: "no-repeat",
      WebkitMaskSize: "contain",
    };
    return (
      <span
        aria-hidden="true"
        data-harness={harness}
        className={cn("inline-flex shrink-0 items-center justify-center", className)}
      >
        <span className="block" style={style} />
      </span>
    );
  }
  return (
    <img
      src={mark}
      alt=""
      draggable={false}
      data-harness={harness}
      className={cn("block shrink-0 object-contain", className)}
    />
  );
}
