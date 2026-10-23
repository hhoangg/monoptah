import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TelegramStatus } from "./telegram";
import {
  cachedTelegramStatus,
  invalidateTelegramStatus,
} from "./telegramStatusCache";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const ready: TelegramStatus = {
  configured: true,
  enabled: true,
  botUsername: "bot",
};

describe("telegram status cache", () => {
  beforeEach(() => {
    invalidateTelegramStatus();
    invoke.mockReset();
    invoke.mockResolvedValue(ready);
  });

  it("serves a fresh entry without asking again", async () => {
    await cachedTelegramStatus("/a", 0);
    await cachedTelegramStatus("/a", 30_000);
    expect(invoke).toHaveBeenCalledTimes(1);
    await cachedTelegramStatus("/a", 61_000);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("does not cache a failure", async () => {
    invoke.mockRejectedValueOnce(new Error("offline"));
    expect(await cachedTelegramStatus("/a", 0)).toBeUndefined();
    expect(await cachedTelegramStatus("/a", 1)).toEqual(ready);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("invalidates one project or all of them", async () => {
    await cachedTelegramStatus("/a", 0);
    await cachedTelegramStatus("/b", 0);
    invalidateTelegramStatus("/a");
    await cachedTelegramStatus("/a", 1);
    await cachedTelegramStatus("/b", 1);
    expect(invoke).toHaveBeenCalledTimes(3);
    invalidateTelegramStatus();
    await cachedTelegramStatus("/a", 2);
    await cachedTelegramStatus("/b", 2);
    expect(invoke).toHaveBeenCalledTimes(5);
  });
});
