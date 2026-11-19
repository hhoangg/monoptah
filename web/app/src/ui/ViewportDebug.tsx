// TEMPORARY diagnostic overlay: live viewport and layout numbers for a real iPhone (screenshot them).
// Numbers and enums only: no URLs, ids, names or text. Loaded lazily by ViewportDebugGate, so it never runs by default.
import { useCallback, useEffect, useRef, useState } from "react";
import { keyboardInset, textFieldFocused } from "@/features/composer/useKeyboardInset";

const MAIN_ID = "main-content";
const n = (value: number | undefined | null): string => (typeof value === "number" && Number.isFinite(value) ? String(Math.round(value * 10) / 10) : "n/a");
const rectOf = (el: Element) => el.getBoundingClientRect();

const HEIGHT_PROBES = ["100vh", "100dvh", "100svh", "100lvh"] as const;
const ENV_SIDES = ["top", "right", "bottom", "left"] as const;
const PADDING_KEY = { top: "paddingTop", right: "paddingRight", bottom: "paddingBottom", left: "paddingLeft" } as const;
const hidden = { position: "fixed", visibility: "hidden", pointerEvents: "none", left: 0, top: 0, width: 1 } as const;

const media = (query: string): boolean => (typeof window.matchMedia === "function" ? window.matchMedia(query).matches : false);

function probeNumber(el: Element | undefined, read: (el: Element) => number): number {
  return el ? read(el) : NaN;
}

function chainLine(el: Element): string {
  const style = getComputedStyle(el);
  const rect = rectOf(el);
  const classes = Array.from(el.classList);
  const first = classes.slice(0, 2).map((c) => c.slice(0, 24)).join(".");
  const heights = classes.filter((c) => /^(min-|max-)?h-/.test(c)).join(",");
  const pb = parseFloat(style.paddingBottom) || 0;
  const mb = parseFloat(style.marginBottom) || 0;
  return `${el.tagName.toLowerCase()}${first ? "." + first : ""} t${n(rect.top)} b${n(rect.bottom)} h${n(rect.height)} css:${style.height}${heights ? " [" + heights + "]" : ""}${pb ? " pb" + n(pb) : ""}${mb ? " mb" + n(mb) : ""}`;
}

function biggestScroller(main: Element): Element | null {
  let best: Element | null = null;
  let scanned = 0;
  for (const el of main.querySelectorAll("*")) {
    if (++scanned > 1500) break;
    const overflowY = getComputedStyle(el).overflowY;
    if ((overflowY === "auto" || overflowY === "scroll") && (!best || el.scrollHeight > best.scrollHeight)) best = el;
  }
  return best;
}

function snapshot(probe: HTMLElement | null): string[] {
  const vv = window.visualViewport;
  const de = document.documentElement;
  const out: string[] = [];
  out.push(`inner ${n(window.innerWidth)}x${n(window.innerHeight)} docClientH ${n(de.clientHeight)} outerH ${n(window.outerHeight)}`);
  out.push(vv ? `vv ${n(vv.width)}x${n(vv.height)} offTop ${n(vv.offsetTop)} pageTop ${n(vv.pageTop)} scale ${n(vv.scale)}` : "vv n/a");
  out.push(`screen ${n(screen.width)}x${n(screen.height)} avail ${n(screen.availHeight)} dpr ${n(window.devicePixelRatio)}`);
  if (probe) {
    const env = ENV_SIDES.map((side, i) => `${side.slice(0, 1)}${n(probeNumber(probe.children[i], (el) => parseFloat(getComputedStyle(el)[PADDING_KEY[side]])))}`).join(" ");
    const units = HEIGHT_PROBES.map((u, i) => `${u.slice(3)} ${n(probeNumber(probe.children[ENV_SIDES.length + i], (el) => rectOf(el).height))}`).join(" ");
    out.push(`env ${env}`);
    out.push(`units ${units}`);
  }
  const nav = navigator as Navigator & { standalone?: boolean };
  out.push(`mode ${media("(display-mode: standalone)") ? "standalone" : "browser"} nav.standalone ${String(nav.standalone)} dark ${String(media("(prefers-color-scheme: dark)"))}`);
  out.push(`ua ${(/OS [\d_]+/.exec(navigator.userAgent)?.[0] ?? "?")}`);
  const typing = textFieldFocused();
  const raw = vv ? keyboardInset(window.innerHeight, vv) : 0;
  out.push(`kbd typing ${String(typing)} inset ${raw} effective ${typing ? raw : 0}`);
  const composer = document.querySelector('[data-testid="composer"]');
  if (composer) {
    const rect = rectOf(composer);
    const style = getComputedStyle(composer);
    const vvGap = vv ? vv.height + vv.offsetTop - rect.bottom : NaN;
    out.push(`composer b${n(rect.bottom)} h${n(rect.height)} pb ${style.paddingBottom} mb ${style.marginBottom} gap(inner) ${n(window.innerHeight - rect.bottom)} gap(vv) ${n(vvGap)}`);
  } else out.push("composer none");
  const region = document.querySelector('[data-testid="composer-region"]');
  out.push(region ? `composer-region b${n(rectOf(region).bottom)} h${n(rectOf(region).height)}` : "composer-region none");
  const main = document.getElementById(MAIN_ID);
  if (main) {
    const rect = rectOf(main);
    out.push(`main t${n(rect.top)} b${n(rect.bottom)} h${n(rect.height)} clientH ${n(main.clientHeight)} scrollTop ${n(main.scrollTop)} scrollH ${n(main.scrollHeight)} css ${getComputedStyle(main).height}`);
    const scroller = biggestScroller(main);
    out.push(scroller ? `scroller h${n(rectOf(scroller).height)} clientH ${n(scroller.clientHeight)} scrollTop ${n(scroller.scrollTop)} scrollH ${n(scroller.scrollHeight)} b${n(rectOf(scroller).bottom)}` : "scroller none");
  } else out.push("main none");
  for (const el of [de, document.body]) {
    const style = getComputedStyle(el);
    out.push(`${el.tagName.toLowerCase()} h${n(rectOf(el).height)} clientH ${n(el.clientHeight)} scrollH ${n(el.scrollHeight)} css ${style.height} min ${style.minHeight}`);
  }
  out.push(`scrollingEl.scrollTop ${n(document.scrollingElement?.scrollTop)} html.scrollTop ${n(de.scrollTop)} body.scrollTop ${n(document.body.scrollTop)} scrollY ${n(window.scrollY)}`);
  if (composer) {
    out.push("chain (composer up):");
    for (let el: Element | null = composer; el; el = el.parentElement) out.push(chainLine(el));
  }
  return out;
}

export default function ViewportDebug() {
  const probeRef = useRef<HTMLDivElement>(null);
  const [lines, setLines] = useState<string[]>([]);
  const [tick, setTick] = useState(0);
  const [shown, setShown] = useState(true);
  const refresh = useCallback(() => {
    setLines(snapshot(probeRef.current));
    setTick((t) => t + 1);
  }, []);
  useEffect(() => {
    // Events coalesce into one snapshot per frame, so the overlay adds little work while the keyboard animates.
    let frame = 0;
    const schedule = () => {
      if (!frame) frame = window.requestAnimationFrame(() => {
        frame = 0;
        refresh();
      });
    };
    refresh();
    const vv = window.visualViewport;
    vv?.addEventListener("resize", schedule);
    vv?.addEventListener("scroll", schedule);
    window.addEventListener("resize", schedule);
    window.addEventListener("scroll", schedule);
    window.addEventListener("orientationchange", schedule);
    document.addEventListener("focusin", schedule);
    document.addEventListener("focusout", schedule);
    const interval = window.setInterval(refresh, 500);
    return () => {
      vv?.removeEventListener("resize", schedule);
      vv?.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", schedule);
      window.removeEventListener("orientationchange", schedule);
      document.removeEventListener("focusin", schedule);
      document.removeEventListener("focusout", schedule);
      window.clearInterval(interval);
      window.cancelAnimationFrame(frame);
    };
  }, [refresh]);
  const text = `#${tick} ${new Date().toISOString().slice(11, 23)}\n${lines.join("\n")}`;
  return (
    <div className="pointer-events-none fixed left-1 z-[9999] max-w-[95vw] font-mono text-[11px] leading-tight text-white" style={{ top: "calc(env(safe-area-inset-top) + 4px)" }} data-testid="viewport-debug">
      <div ref={probeRef} aria-hidden="true">
        {ENV_SIDES.map((side) => (
          <div key={side} style={{ ...hidden, [PADDING_KEY[side]]: `env(safe-area-inset-${side})` }} />
        ))}
        {HEIGHT_PROBES.map((unit) => (
          <div key={unit} style={{ ...hidden, height: unit }} />
        ))}
      </div>
      <div className="pointer-events-auto mb-0.5 flex gap-1">
        <button type="button" className="rounded bg-black/70 px-1.5 py-0.5" onClick={() => setShown((s) => !s)}>
          {shown ? "hide" : "show"}
        </button>
        <button type="button" className="rounded bg-black/70 px-1.5 py-0.5" onClick={() => void navigator.clipboard?.writeText(text).catch(() => undefined)}>
          copy
        </button>
      </div>
      {shown ? <pre className="m-0 whitespace-pre-wrap break-all rounded bg-black/70 p-1 font-mono">{text}</pre> : null}
    </div>
  );
}
