import { beforeEach, describe, expect, it, vi } from "vitest";
import { newSession } from "../model/session";
import type { SessionRecord } from "./sessionStore";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

const { getSession, persistFingerprint, sanitizeSessionForPersist } =
  await import("./sessionStore");

function saved(surface: "chat" | "tui") {
  const session = newSession(
    "claude",
    "/repo",
    undefined,
    undefined,
    undefined,
    {
      surface,
    },
  );
  session.blocks = [{ id: "u1", role: "user", text: "hello" }];
  return session;
}

describe("session surface persistence", () => {
  beforeEach(() => invoke.mockReset());

  it("saves a terminal session and loads it back as a terminal session", async () => {
    const session = saved("tui");
    const payload = sanitizeSessionForPersist(session);
    expect(payload.surface).toBe("tui");
    // The Rust store hands the stored payload back as the record.
    invoke.mockResolvedValue({
      ...payload,
      createdAt: 1,
      updatedAt: 1,
    } satisfies Record<string, unknown>);
    expect((await getSession(session.id))?.surface).toBe("tui");
  });

  it("saves chat explicitly and loads it back as chat", async () => {
    const session = saved("chat");
    const payload = sanitizeSessionForPersist(session);
    expect(payload.surface).toBe("chat");
    invoke.mockResolvedValue({ ...payload, createdAt: 1, updatedAt: 1 });
    expect((await getSession(session.id))?.surface).toBe("chat");
  });

  it("loads a row written before the surface column existed as chat", async () => {
    const session = saved("chat");
    const { surface: _surface, ...legacy } = sanitizeSessionForPersist(session);
    invoke.mockResolvedValue({
      ...legacy,
      createdAt: 1,
      updatedAt: 1,
    } as unknown as SessionRecord);
    expect((await getSession(session.id))?.surface).toBe("chat");
    invoke.mockResolvedValue({
      ...legacy,
      surface: null,
      createdAt: 1,
      updatedAt: 1,
    } as unknown as SessionRecord);
    expect((await getSession(session.id))?.surface).toBe("chat");
  });

  it("treats an unknown stored value as chat", async () => {
    const session = saved("chat");
    invoke.mockResolvedValue({
      ...sanitizeSessionForPersist(session),
      surface: "pty",
      createdAt: 1,
      updatedAt: 1,
    });
    expect((await getSession(session.id))?.surface).toBe("chat");
  });

  it("treats a session with no surface as chat when saving", () => {
    const { surface: _surface, ...bare } = saved("chat");
    expect(sanitizeSessionForPersist(bare).surface).toBe("chat");
  });

  it("changes the fingerprint when the surface changes", () => {
    expect(persistFingerprint(saved("tui"))).not.toBe(
      persistFingerprint({ ...saved("tui"), surface: "chat" }),
    );
  });
});
