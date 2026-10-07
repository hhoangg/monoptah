import { describe, expect, it, vi } from "vitest";
import type { InboxComposerCard } from "../../inbox/model/githubTasks";
import { newSession, type Session } from "./session";

const paths = vi.hoisted(() => ({ configured: {} as Record<string, string> }));
vi.mock("../../providers/model/providerBinaryPaths", () => ({
  runtimeProviderBinaryPath: (provider: string) =>
    paths.configured[provider] ?? null,
}));
import {
  TUI_EARLY_EXIT_MS,
  applyInboxStart,
  applyTuiSessionPatch,
  canRetitleFromTui,
  canStartFreshAfter,
  cleanTuiTitle,
  isNoConversationOutput,
  isTuiSession,
  plainTerminalText,
  tuiBinaryIdentity,
  tuiExitNotice,
  tuiLaunchCwd,
  tuiProviderAccount,
  tuiPtyId,
} from "./tuiSession";

const tui = (harness: Parameters<typeof newSession>[0] = "claude"): Session =>
  newSession(harness, "/work/app", undefined, undefined, undefined, {
    surface: "tui",
  });

const card: InboxComposerCard = {
  provider: "github",
  kind: "issue",
  identifier: "#12",
  title: "Fix the bug",
  url: "https://example.test/12",
  source: "acme/app",
  labels: [],
  prompt: "  Please fix issue 12.  \n",
};

describe("terminal session identity", () => {
  it("recognises only a terminal surface", () => {
    expect(isTuiSession(tui())).toBe(true);
    expect(isTuiSession(newSession("claude", "/work/app"))).toBe(false);
    expect(isTuiSession(undefined)).toBe(false);
  });

  it("keeps the PTY apart from file-pane terminals", () => {
    expect(tuiPtyId("abc")).toBe("tui:abc");
  });

  it("launches in the worktree when there is one and it still exists", () => {
    expect(tuiLaunchCwd({ cwd: "/work/app" })).toBe("/work/app");
    expect(tuiLaunchCwd({ cwd: "/work/app", worktreeCwd: "/work/wt" })).toBe(
      "/work/wt",
    );
    expect(
      tuiLaunchCwd({
        cwd: "/work/app",
        worktreeCwd: "/work/wt",
        worktreeRemoved: true,
      }),
    ).toBe("/work/app");
  });
});

describe("binary identity for the PTY launch", () => {
  it("names the provider alone when no binary path is configured", () => {
    expect(tuiBinaryIdentity("codex")).toEqual({ binaryProvider: "codex" });
  });

  it("sends the configured binary path so the backend can check it", () => {
    paths.configured.claude = "/opt/claude";
    try {
      expect(tuiBinaryIdentity("claude")).toEqual({
        binaryProvider: "claude",
        binaryPath: "/opt/claude",
      });
    } finally {
      delete paths.configured.claude;
    }
  });

  it("never sends antigravity's configured ACP server as the agy path", () => {
    paths.configured.antigravity = "/opt/agy_acp_server.par";
    try {
      expect(tuiBinaryIdentity("antigravity")).toEqual({
        binaryProvider: "antigravity",
      });
    } finally {
      delete paths.configured.antigravity;
    }
  });
});

describe("provider account for the PTY", () => {
  it("sends a named Claude or Codex account", () => {
    expect(tuiProviderAccount("claude", "work")).toEqual({
      provider: "claude",
      id: "work",
    });
    expect(tuiProviderAccount("codex", "work")).toEqual({
      provider: "codex",
      id: "work",
    });
  });

  it("sends nothing for the default account or an unset one", () => {
    expect(tuiProviderAccount("claude", "default")).toBeUndefined();
    expect(tuiProviderAccount("claude", undefined)).toBeUndefined();
  });

  it("sends nothing for providers without accounts", () => {
    expect(tuiProviderAccount("cursor", "work")).toBeUndefined();
  });
});

describe("applyTuiSessionPatch", () => {
  it("saves a new conversation id and account", () => {
    const next = applyTuiSessionPatch(tui(), {
      providerSessionId: "id-1",
      providerAccountId: "work",
    });
    expect(next.providerSessionId).toBe("id-1");
    expect(next.providerAccountId).toBe("work");
  });

  it("returns the same object when nothing changes", () => {
    const session = { ...tui(), providerSessionId: "id-1" };
    expect(applyTuiSessionPatch(session, { providerSessionId: "id-1" })).toBe(
      session,
    );
    expect(applyTuiSessionPatch(session, {})).toBe(session);
  });

  it("forgets the conversation on null and leaves the account alone", () => {
    const session = {
      ...tui(),
      providerSessionId: "id-1",
      providerAccountId: "work",
    };
    const next = applyTuiSessionPatch(session, { providerSessionId: null });
    expect(next.providerSessionId).toBeUndefined();
    expect(next.providerAccountId).toBe("work");
  });
});

describe("tab title from the terminal", () => {
  it("drops status glyphs and spinner frames", () => {
    expect(cleanTuiTitle("✳ Claude Code")).toBe("Claude Code");
    expect(cleanTuiTitle("⠂ Fix the login bug")).toBe("Fix the login bug");
    expect(cleanTuiTitle("  \u001b spaced   out ")).toBe("spaced out");
  });

  it("returns nothing for a title with no words", () => {
    expect(cleanTuiTitle("✳")).toBe("");
    expect(cleanTuiTitle("")).toBe("");
  });

  it("caps a long title", () => {
    const cleaned = cleanTuiTitle("a".repeat(200));
    expect(cleaned.length).toBe(80);
    expect(cleaned.endsWith("…")).toBe(true);
  });

  it("retitles a placeholder tab but not one the app or user named", () => {
    const fresh = tui();
    expect(canRetitleFromTui(fresh, undefined)).toBe(true);
    expect(
      canRetitleFromTui({ ...fresh, title: "#12 Fix the bug" }, undefined),
    ).toBe(false);
    expect(
      canRetitleFromTui({ ...fresh, title: "Renamed by me" }, "Claude Code"),
    ).toBe(false);
  });

  it("keeps following the terminal once it set the title itself", () => {
    expect(
      canRetitleFromTui({ ...tui(), title: "Claude Code" }, "Claude Code"),
    ).toBe(true);
  });
});

describe("exit notice", () => {
  it("names the exit code", () => {
    expect(tuiExitNotice("claude", { code: 130, early: false })).toEqual({
      message: "Process exited (code 130)",
    });
  });

  it("omits the code when there is none", () => {
    expect(tuiExitNotice("claude", { code: null, early: false }).message).toBe(
      "Process exited",
    );
  });

  it("explains an exit right after launch", () => {
    const notice = tuiExitNotice("codex", { code: 1, early: true });
    expect(notice.message).toBe("Process exited (code 1)");
    expect(notice.hint).toContain("Codex");
    expect(notice.hint).toContain("interactive mode");
  });

  it("shows the reason a process could not start", () => {
    expect(
      tuiExitNotice("grok", {
        code: null,
        error: "Failed to start grok: not found",
        early: true,
      }),
    ).toEqual({
      message: "Could not start Grok Build",
      hint: "Failed to start grok: not found",
    });
  });

  it("treats five seconds as the early window", () => {
    expect(TUI_EARLY_EXIT_MS).toBe(5000);
  });
});

describe("starting fresh after a failed resume", () => {
  const early = { code: 1, early: true };

  it("is offered when a resuming provider exits at once", () => {
    expect(canStartFreshAfter("claude", early, true)).toBe(true);
  });

  it("is not offered for a late exit, a first launch or a start failure", () => {
    expect(canStartFreshAfter("claude", { code: 0, early: false }, true)).toBe(
      false,
    );
    expect(canStartFreshAfter("claude", early, false)).toBe(false);
    expect(
      canStartFreshAfter(
        "claude",
        { code: null, error: "no such file", early: true },
        true,
      ),
    ).toBe(false);
  });

  it("is not offered for a provider that cannot resume", () => {
    expect(canStartFreshAfter("codex", early, true)).toBe(false);
  });
});

describe("Inbox start work", () => {
  it("keeps the card above the composer for a chat session", () => {
    const next = applyInboxStart(newSession("claude", "/work/app"), card);
    expect(next.inboxCard).toBe(card);
    expect(next.initialPrompt).toBeUndefined();
    expect(next.surface).toBe("chat");
  });

  it("hands the composed prompt to a terminal session as its first prompt", () => {
    const next = applyInboxStart(tui(), card);
    expect(next.initialPrompt).toBe("Please fix issue 12.");
    expect(next.inboxCard).toBeUndefined();
    expect(next.composerSeed).toBeUndefined();
    expect(next.surface).toBe("tui");
  });

  it("opens a chat when the provider's terminal cannot take a first prompt", () => {
    const next = applyInboxStart(tui("codex"), card);
    expect(next.surface).toBe("chat");
    expect(next.inboxCard).toBe(card);
    expect(next.initialPrompt).toBeUndefined();
  });

  it("starts a terminal session without a prompt when the card has none", () => {
    const next = applyInboxStart(tui(), { ...card, prompt: "  " });
    expect(next.initialPrompt).toBeUndefined();
    expect(next.surface).toBe("tui");
  });
});

// Captured from `claude 2.1.291 --resume 11111111-... ` run in a PTY: each
// word sits at an absolute column with no space between.
const REAL_NO_CONVERSATION =
  "\u001b7\u001b[r\u001b8\u001b[?25h\u001b[?25l\u001b[?2004h\u001b[?2031h\u001b[?1004h\u001b]11;?\u0007\u001b[c\u001b[>0q\u001b[?u\u001b[c\u001b[>4m\u001b[?1004l\u001b[?2031l\u001b[?2004lNo\u001b[4Gconversation\u001b[17Gfound\u001b[23Gwith\u001b[28Gsession\u001b[36GID:\u001b[40G11111111-2222-3333-4444-555555555555\r\r\n\u001b[?25h\u001b(B\u000f\u001b[?1016l\u001b]0;\u0007";

describe("recognising Claude's no-conversation exit", () => {
  it("reads the real output, where words are placed by cursor moves", () => {
    expect(plainTerminalText(REAL_NO_CONVERSATION)).toBe(
      "No conversation found with session ID: 11111111-2222-3333-4444-555555555555",
    );
    expect(isNoConversationOutput(REAL_NO_CONVERSATION)).toBe(true);
  });

  it("reads the plain print-mode line", () => {
    expect(
      isNoConversationOutput("No conversation found with session ID: 1111\r\n"),
    ).toBe(true);
  });

  it("does not match other failures", () => {
    expect(
      isNoConversationOutput("Error: Session ID abc is already in use."),
    ).toBe(false);
    expect(isNoConversationOutput("command not found: claude")).toBe(false);
    expect(isNoConversationOutput("")).toBe(false);
    expect(isNoConversationOutput(undefined)).toBe(false);
  });
});
