import { describe, expect, it, vi } from "vitest";

const core = vi.hoisted(() => ({ invoke: vi.fn(async () => undefined) }));
vi.mock("@tauri-apps/api/core", () => core);
import { decodePtyChunk, spawnPty, trimReplay } from "./pty";

const KB = 1024;

describe("decodePtyChunk", () => {
  it("decodes a valid base64 payload", () => {
    // "hi" in base64
    const chunk = decodePtyChunk("aGk=");
    expect(chunk).not.toBeNull();
    expect(Array.from(chunk!)).toEqual([104, 105]);
  });

  it("returns null instead of throwing on a malformed payload", () => {
    expect(() => decodePtyChunk("not valid base64!!!")).not.toThrow();
    expect(decodePtyChunk("not valid base64!!!")).toBeNull();
  });
});

describe("trimReplay", () => {
  it("keeps a small buffer whole", () => {
    const sizes = [KB, KB, KB];
    expect(trimReplay(sizes, 3 * KB)).toEqual({ drop: 0, bytes: 3 * KB });
  });

  it("drops oldest chunks once the byte budget is exceeded", () => {
    // Ten 32KB chunks is 320KB, over the 256KB budget.
    const sizes = Array(10).fill(32 * KB);
    const { drop, bytes } = trimReplay(sizes, 320 * KB);
    expect(drop).toBe(2);
    expect(bytes).toBe(256 * KB);
  });

  it("bounds a flood of tiny chunks by count", () => {
    const sizes = Array(250).fill(4);
    const { drop } = trimReplay(sizes, 1000);
    expect(sizes.length - drop).toBe(200);
  });

  it("keeps the newest chunk even when it alone exceeds the budget", () => {
    const sizes = [KB, 512 * KB];
    const { drop, bytes } = trimReplay(sizes, 513 * KB);
    expect(drop).toBe(1);
    expect(bytes).toBe(512 * KB);
  });

  it("never drops the only chunk", () => {
    const sizes = [512 * KB];
    expect(trimReplay(sizes, 512 * KB)).toEqual({ drop: 0, bytes: 512 * KB });
  });
});

describe("spawnPty", () => {
  it("sends no launch key for a plain shell", async () => {
    core.invoke.mockClear();
    await spawnPty("t", "/tmp", 80, 24);
    expect(core.invoke).toHaveBeenCalledTimes(1);
    const [command, payload] = core.invoke.mock.calls[0] as unknown as [
      string,
      Record<string, unknown>,
    ];
    expect(command).toBe("pty_spawn");
    expect(JSON.parse(JSON.stringify(payload))).toEqual({
      id: "t",
      cwd: "/tmp",
      cols: 80,
      rows: 24,
    });
  });

  it("sends the launch under the camelCase wire names", async () => {
    core.invoke.mockClear();
    const launch = {
      program: "/opt/bin/codex",
      args: ["--flag"],
      providerAccount: { provider: "codex" as const, id: "work" },
    };
    await spawnPty("t", "/tmp", 80, 24, launch);
    expect(core.invoke).toHaveBeenCalledWith("pty_spawn", {
      id: "t",
      cwd: "/tmp",
      cols: 80,
      rows: 24,
      launch,
    });
  });
});
