// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GhNotice } from "./GhNotice";

describe("GhNotice", () => {
  it("explains the missing review points with the reason", () => {
    const markup = renderToStaticMarkup(
      createElement(GhNotice, {
        note: "GitHub CLI is not signed in. Run `gh auth login`.",
        onRetry: () => {},
      }),
    );
    expect(markup).toContain(
      "Review points are not counted because GitHub data is unavailable: GitHub CLI is not signed in. Run `gh auth login`",
    );
    expect(markup).toContain("PRs are inferred from git history.");
    expect(markup).not.toContain("..");
  });

  it("still reads well without a note", () => {
    const markup = renderToStaticMarkup(
      createElement(GhNotice, { onRetry: () => {} }),
    );
    expect(markup).toContain("GitHub data is unavailable. PRs are inferred");
  });
});

describe("GhNotice with GitHub available", () => {
  it("shows the note as plain information, without the review warning", () => {
    const markup = renderToStaticMarkup(
      createElement(GhNotice, {
        available: true,
        note: "GitHub returned no pull requests for this period, so review points may be missing.",
        onRetry: () => {},
      }),
    );
    expect(markup).toContain(
      "GitHub returned no pull requests for this period, so review points may be missing.",
    );
    expect(markup).not.toContain("not counted");
    expect(markup).not.toContain("unavailable");
    expect(markup).not.toContain("amber");
    expect(markup).toContain('role="status"');
    expect(markup).toContain("Retry");
  });
});

describe("GhNotice retry", () => {
  let container: HTMLDivElement;
  let root: Root;
  beforeEach(() => {
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  it("calls onRetry from the Retry button and disables it while retrying", async () => {
    const onRetry = vi.fn();
    await act(async () =>
      root.render(createElement(GhNotice, { note: "x", onRetry })),
    );
    const retry = container.querySelector("button") as HTMLButtonElement;
    expect(retry.textContent).toContain("Retry");
    await act(async () => retry.click());
    expect(onRetry).toHaveBeenCalledTimes(1);

    await act(async () =>
      root.render(
        createElement(GhNotice, { note: "x", onRetry, retrying: true }),
      ),
    );
    expect(container.querySelector("button")?.disabled).toBe(true);
  });
});

describe("GhNotice with incomplete GitHub data", () => {
  it("says the period is incomplete and offers Retry GitHub", () => {
    const markup = renderToStaticMarkup(
      createElement(GhNotice, {
        incomplete: true,
        available: true,
        note: "GitHub rate limit reached.",
        onRetry: () => {},
      }),
    );
    expect(markup).toContain("GitHub data is incomplete for this period.");
    expect(markup).toContain("GitHub rate limit reached.");
    expect(markup).toContain("Retry GitHub");
    expect(markup).toContain("amber");
    expect(markup).not.toContain("..");
  });

  it("reads well without a note, and says review points are lost when GitHub is unavailable", () => {
    const partial = renderToStaticMarkup(
      createElement(GhNotice, {
        incomplete: true,
        available: true,
        onRetry: () => {},
      }),
    );
    expect(partial).toContain("GitHub data is incomplete for this period.");
    expect(partial).not.toContain("not counted");
    const down = renderToStaticMarkup(
      createElement(GhNotice, { incomplete: true, onRetry: () => {} }),
    );
    expect(down).toContain("Review points are not counted.");
  });
});
