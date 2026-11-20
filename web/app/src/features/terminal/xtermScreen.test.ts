// xterm itself in jsdom: it has no canvas and no layout, so only what jsdom can run is checked (the DOM renderer needs no canvas).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createXtermScreen, type XtermScreen } from "./xtermScreen";

let screen: XtermScreen | null = null;
beforeEach(() => {
  // xterm asks the window for the color scheme and the device pixel ratio; jsdom has no matchMedia.
  window.matchMedia = ((query: string) => ({ matches: false, media: query, onchange: null, addEventListener: () => {}, removeEventListener: () => {}, addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false })) as unknown as typeof window.matchMedia;
});
afterEach(() => {
  screen?.dispose();
  screen = null;
  document.body.replaceChildren();
});

async function build(): Promise<{ host: HTMLElement; data: string[]; sizes: Array<[number, number]> }> {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const data: string[] = [];
  const sizes: Array<[number, number]> = [];
  screen = await createXtermScreen(host, { onData: (d) => data.push(d), onResize: (c, r) => sizes.push([c, r]) });
  return { host, data, sizes };
}

describe("createXtermScreen (jsdom)", () => {
  it("draws bytes as text nodes, calls back when parsed, and never turns output into markup", async () => {
    const built = await build();
    const sc = screen!;
    const hostile = "\x1b]0;<img src=x onerror=alert(1)>\x07<script>window.__pwned=1</script><b>bold</b>\r\n";
    await new Promise<void>((done) => sc.write(new TextEncoder().encode(hostile), done));
    expect((window as unknown as Record<string, unknown>)["__pwned"]).toBeUndefined();
    expect(built.host.querySelector("script, img, b")).toBeNull();
    expect(document.title).not.toContain("img");
    expect(sc.size().cols).toBeGreaterThan(0);
  });

  it("links the program prints activate nothing", async () => {
    const built = await build();
    const sc = screen!;
    const open = vi.spyOn(window, "open").mockReturnValue(null);
    await new Promise<void>((done) => sc.write(new TextEncoder().encode("\x1b]8;;https://example.invalid/\x07click\x1b]8;;\x07\r\n"), done));
    for (const element of built.host.querySelectorAll("a")) (element as HTMLElement).click();
    expect(open).not.toHaveBeenCalled();
    open.mockRestore();
  });

  it("OSC 52 does not touch the clipboard", async () => {
    await build();
    const sc = screen!;
    const write = vi.fn();
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: write, write } });
    await new Promise<void>((done) => sc.write(new TextEncoder().encode("\x1b]52;c;aGVsbG8=\x07"), done));
    expect(write).not.toHaveBeenCalled();
  });

  it("calls the pending write callbacks when it is disposed, so the read loop never stalls", async () => {
    await build();
    const sc = screen!;
    const done = vi.fn();
    sc.write(new TextEncoder().encode("x".repeat(10)), done);
    sc.dispose();
    await Promise.resolve();
    expect(done).toHaveBeenCalled();
    const after = vi.fn();
    sc.write(new Uint8Array(1), after);
    expect(after).toHaveBeenCalled();
  });
});
