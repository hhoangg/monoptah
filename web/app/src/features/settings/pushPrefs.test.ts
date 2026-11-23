import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { createQueryClient } from "@/lib/queryClient";
import { pauseAll, resumeAll } from "./pushPrefs";

let restore: (() => void) | undefined;
afterEach(() => restore?.());

function limited() {
  return new Response(JSON.stringify({ error: "Too many requests", code: "rate_limited" }), {
    status: 429,
    headers: { "Content-Type": "application/json", "Retry-After": "30" },
  });
}

describe("Pause all and Resume all", () => {
  it.each([
    ["pauseAll", pauseAll],
    ["resumeAll", resumeAll],
  ])("%s fails at once on a 429 instead of waiting for the retries", async (_name, run) => {
    const transport = vi.fn(async () => limited());
    restore = setApiTransport(transport);
    await expect(run(createQueryClient())).rejects.toMatchObject({ status: 429, code: "rate_limited" });
    expect(transport).toHaveBeenCalledTimes(1);
  });

  it("keeps its single retry for another error", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const transport = vi.fn(async () => Response.json({ error: "Boom", code: "internal" }, { status: 500 }));
      restore = setApiTransport(transport);
      const result = expect(pauseAll(createQueryClient())).rejects.toMatchObject({ status: 500 });
      await vi.advanceTimersByTimeAsync(5_000);
      await result;
      expect(transport).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });
});
