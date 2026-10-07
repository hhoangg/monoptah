import { beforeEach, describe, expect, it, vi } from "vitest";
import { newSession } from "../model/session";
import type { SessionRecord } from "./sessionStore";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

const {
  getSession,
  persistFingerprint,
  sanitizeSessionForPersist,
  shouldPersistSession,
  upsertSession,
} = await import("./sessionStore");

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

describe("persisting a terminal session with no chat blocks", () => {
  beforeEach(() => invoke.mockReset());

  function blank(surface: "chat" | "tui") {
    return { ...saved(surface), blocks: [], providerSessionId: "conv-1" };
  }

  it("keeps a terminal session that has a conversation id", () => {
    expect(shouldPersistSession(blank("tui"))).toBe(true);
  });

  it("leaves a terminal session with no conversation id unsaved", () => {
    expect(
      shouldPersistSession({ ...blank("tui"), providerSessionId: undefined }),
    ).toBe(false);
    expect(
      shouldPersistSession({ ...blank("tui"), providerSessionId: "" }),
    ).toBe(false);
  });

  it("does not write a terminal session with no conversation id", async () => {
    expect(
      await upsertSession({ ...blank("tui"), providerSessionId: undefined }),
    ).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("still leaves an empty chat tab unsaved", () => {
    expect(shouldPersistSession(blank("chat"))).toBe(false);
  });

  it("still never saves a terminal session in a remote or home project", () => {
    expect(
      shouldPersistSession({ ...blank("tui"), cwd: "remote://host/repo" }),
    ).toBe(false);
    expect(shouldPersistSession({ ...blank("tui"), cwd: "~" })).toBe(false);
    expect(shouldPersistSession({ ...blank("tui"), ephemeral: true })).toBe(
      false,
    );
  });

  it("writes the row with its surface and conversation id", async () => {
    invoke.mockResolvedValue({ id: "x" });
    const session = blank("tui");
    await upsertSession(session);
    expect(invoke).toHaveBeenCalledTimes(1);
    const [command, args] = invoke.mock.calls[0];
    expect(command).toBe("session_upsert");
    expect(args.session).toMatchObject({
      id: session.id,
      surface: "tui",
      providerSessionId: "conv-1",
      blocks: [],
    });
  });

  it("does not write an empty chat tab", async () => {
    expect(await upsertSession(blank("chat"))).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });
});
