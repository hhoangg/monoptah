import { describe, expect, it } from "vitest";
import { oneLine } from "./WorkspaceNav";

describe("oneLine", () => {
  it("shows a reaction as its emoji and drops markdown marks", () => {
    expect(oneLine("<reaction> 👍 </reaction>")).toBe("Reacted 👍");
    expect(oneLine("**Done** with [the docs](https://a.dev)")).toBe("Done with the docs");
  });

  it("stays fast on a hostile reaction with a huge run of spaces", () => {
    const started = performance.now();
    oneLine("<reaction>" + " ".repeat(40_000) + "a");
    oneLine("<reaction>" + " ".repeat(40_000) + "a</reaction>");
    expect(performance.now() - started).toBeLessThan(200);
  });
});
