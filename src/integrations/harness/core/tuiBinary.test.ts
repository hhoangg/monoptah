import { describe, expect, it, vi } from "vitest";
import { HARNESSES } from "../../../features/sessions/model/session";
import { resolveTuiBinary } from "./tuiBinary";

const child = vi.hoisted(() => {
  const resolver = (name: string) => vi.fn(async () => ({ path: `/bin/${name}` }));
  return {
    resolveClaudeBinary: resolver("claude"),
    resolveCodexBinary: resolver("codex"),
    resolveCursorBinary: resolver("cursor"),
    resolveGrokBinary: resolver("grok"),
    resolveOpenCodeBinary: resolver("opencode"),
    resolvePiBinary: resolver("pi"),
    resolveOmpBinary: resolver("omp"),
    resolveFxBinary: resolver("fx"),
    resolveHermesBinary: resolver("hermes"),
    resolveDevinBinary: resolver("devin"),
    resolveAntigravityBinary: vi.fn(async () => ({
      path: "/bin/agy_acp_server.par",
      args: ["--acp"],
    })),
  };
});
vi.mock("./child", () => child);

describe("resolveTuiBinary", () => {
  it.each(HARNESSES.filter((harness) => harness !== "antigravity"))(
    "resolves %s through its own binary resolver",
    async (harness) => {
      expect(await resolveTuiBinary(harness)).toBe(`/bin/${harness}`);
    },
  );

  it("launches the interactive agy CLI, never the ACP server", async () => {
    expect(await resolveTuiBinary("antigravity")).toBe("agy");
    expect(child.resolveAntigravityBinary).not.toHaveBeenCalled();
  });

  it("surfaces a resolver failure to the caller", async () => {
    child.resolveCodexBinary.mockRejectedValueOnce(new Error("codex missing"));
    await expect(resolveTuiBinary("codex")).rejects.toThrow("codex missing");
  });
});
