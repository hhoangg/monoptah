import { describe, expect, expectTypeOf, it } from "vitest";
import {
  HARNESSES,
  type HarnessId,
  type RuntimeMode,
} from "../../../features/sessions/model/session";
import { buildTuiLaunch, TUI_CAPS, tuiHookSettings } from "./tuiLaunch";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

function session(
  overrides: Partial<{
    harness: HarnessId;
    model: string;
    runtimeMode: RuntimeMode;
    providerSessionId: string;
  }> = {},
) {
  return {
    harness: "claude" as HarnessId,
    model: "claude:opus-5-5",
    runtimeMode: "supervised" as RuntimeMode,
    ...overrides,
  };
}

const binaryPath = "/opt/bin/cli";

describe("TUI_CAPS", () => {
  it("has an entry for every harness id", () => {
    expect(Object.keys(TUI_CAPS).sort()).toEqual([...HARNESSES].sort());
  });

  it("is keyed by exactly the HarnessId union at the type level", () => {
    expectTypeOf<keyof typeof TUI_CAPS>().toEqualTypeOf<HarnessId>();
  });

  it("only lets Claude resume in v1", () => {
    for (const id of HARNESSES) {
      expect(TUI_CAPS[id].resume).toBe(id === "claude");
    }
  });
});

describe("buildTuiLaunch for a new claude session", () => {
  it("mints a uuid, passes it as --session-id and returns it", () => {
    const launch = buildTuiLaunch(session(), { binaryPath });
    expect(launch.program).toBe(binaryPath);
    expect(launch.providerSessionId).toMatch(UUID);
    const at = launch.args.indexOf("--session-id");
    expect(at).toBeGreaterThanOrEqual(0);
    expect(launch.args[at + 1]).toBe(launch.providerSessionId);
    expect(launch.args).not.toContain("--resume");
  });

  it("mints a different id each launch", () => {
    const a = buildTuiLaunch(session(), { binaryPath });
    const b = buildTuiLaunch(session(), { binaryPath });
    expect(a.providerSessionId).not.toBe(b.providerSessionId);
  });

  it("passes the native model id", () => {
    const { args } = buildTuiLaunch(session(), { binaryPath });
    expect(args[args.indexOf("--model") + 1]).toBe("claude-opus-5-5");
  });

  it("sends the initial prompt as the final positional argument", () => {
    const { args } = buildTuiLaunch(session(), {
      binaryPath,
      initialPrompt: "fix the build",
    });
    expect(args[args.length - 1]).toBe("fix the build");
    expect(args[args.length - 2]).toBe(args[args.indexOf("--session-id") + 1]);
  });

  it("guards a prompt that starts with a dash from flag parsing", () => {
    const { args } = buildTuiLaunch(session(), {
      binaryPath,
      initialPrompt: "--help me",
    });
    expect(args.slice(-2)).toEqual(["--", "--help me"]);
  });

  it("adds no positional for a missing or blank prompt", () => {
    const bare = buildTuiLaunch(session(), { binaryPath });
    const blank = buildTuiLaunch(session(), {
      binaryPath,
      initialPrompt: " \n",
    });
    expect(bare.args.at(-1)).toBe(bare.providerSessionId);
    expect(blank.args.at(-1)).toBe(blank.providerSessionId);
  });
});

describe("buildTuiLaunch for a resumed claude session", () => {
  const resumed = session({ providerSessionId: "prov-123" });

  it("resumes by id and mints nothing", () => {
    const launch = buildTuiLaunch(resumed, { binaryPath });
    expect(launch.args).toContain("--resume");
    expect(launch.args[launch.args.indexOf("--resume") + 1]).toBe("prov-123");
    expect(launch.args).not.toContain("--session-id");
    expect(launch.providerSessionId).toBeUndefined();
    expect("providerSessionId" in launch).toBe(false);
  });

  it("drops the initial prompt", () => {
    const { args } = buildTuiLaunch(resumed, {
      binaryPath,
      initialPrompt: "ignored",
    });
    expect(args).not.toContain("ignored");
    expect(args.at(-1)).toBe("prov-123");
  });
});

describe("buildTuiLaunch permission mode", () => {
  const cases: Array<[RuntimeMode, string]> = [
    ["supervised", "default"],
    ["auto-accept-edits", "acceptEdits"],
    ["auto", "auto"],
    ["full-access", "bypassPermissions"],
  ];

  it.each(cases)("maps %s to --permission-mode %s", (runtimeMode, expected) => {
    const { args } = buildTuiLaunch(session({ runtimeMode }), { binaryPath });
    expect(args[args.indexOf("--permission-mode") + 1]).toBe(expected);
  });
});

describe("buildTuiLaunch for providers without TUI flags", () => {
  const others = HARNESSES.filter((id) => id !== "claude");

  it.each(others)("launches the bare %s binary", (harness) => {
    const launch = buildTuiLaunch(
      session({
        harness,
        model: `${harness}:some-model`,
        runtimeMode: "full-access",
        providerSessionId: "prov-123",
      }),
      { binaryPath, initialPrompt: "ignored" },
    );
    expect(launch).toEqual({ program: binaryPath, args: [] });
  });
});

describe("buildTuiLaunch is driven by TUI_CAPS", () => {
  it("drops the resume flags when the claude resume cap is switched off", () => {
    const original = TUI_CAPS.claude.resume;
    TUI_CAPS.claude.resume = false;
    try {
      const launch = buildTuiLaunch(
        session({ providerSessionId: "prov-123" }),
        { binaryPath },
      );
      expect(launch.args).not.toContain("--resume");
      expect(launch.args).not.toContain("--session-id");
      expect(launch.providerSessionId).toBeUndefined();
    } finally {
      TUI_CAPS.claude.resume = original;
    }
  });

  it("drops the prompt when the claude initialPrompt cap is switched off", () => {
    const original = TUI_CAPS.claude.initialPrompt;
    TUI_CAPS.claude.initialPrompt = false;
    try {
      const { args } = buildTuiLaunch(session(), {
        binaryPath,
        initialPrompt: "hello",
      });
      expect(args).not.toContain("hello");
    } finally {
      TUI_CAPS.claude.initialPrompt = original;
    }
  });

  it("does not share one caps object between providers", () => {
    expect(TUI_CAPS.codex).not.toBe(TUI_CAPS.cursor);
  });
});

describe("claiming a saved id for a new conversation", () => {
  const session = {
    harness: "claude" as const,
    model: "claude-sonnet-4-5",
    runtimeMode: "supervised" as const,
  };

  it("starts the new conversation under the id it was given", () => {
    const launch = buildTuiLaunch(session, {
      binaryPath: "/bin/claude",
      newProviderSessionId: "saved-id",
    });
    expect(launch.providerSessionId).toBe("saved-id");
    expect(launch.args).toContain("--session-id");
    expect(launch.args[launch.args.indexOf("--session-id") + 1]).toBe(
      "saved-id",
    );
    expect(launch.args).not.toContain("--resume");
  });

  it("is ignored when the session already has an id to resume", () => {
    const launch = buildTuiLaunch(
      { ...session, providerSessionId: "existing" },
      { binaryPath: "/bin/claude", newProviderSessionId: "other" },
    );
    expect(launch.args).toContain("--resume");
    expect(launch.providerSessionId).toBeUndefined();
  });

  it("changes nothing for a provider that cannot resume", () => {
    expect(
      buildTuiLaunch(
        { ...session, harness: "codex" },
        { binaryPath: "/bin/codex", newProviderSessionId: "saved-id" },
      ),
    ).toEqual({ program: "/bin/codex", args: [] });
  });
});

describe("buildTuiLaunch hook injection", () => {
  const hookNotify = {
    cliPath: "/Applications/Monoptah.app/Contents/MacOS/monocode",
    spoolDir: "/Users/me/Library/Application Support/x/tui-hooks",
  };

  it("passes no --settings without hookNotify", () => {
    const { args } = buildTuiLaunch(session(), { binaryPath });
    expect(args).not.toContain("--settings");
  });

  it("passes no --settings on Windows, where no one command suits both shells", () => {
    const { args } = buildTuiLaunch(session(), {
      binaryPath: "C:\\Users\\me\\claude.exe",
      hookNotify: {
        cliPath: "C:\\Program Files\\Monoptah\\monocode.exe",
        spoolDir: "C:\\Users\\me\\AppData\\tui-hooks",
      },
      isWindows: true,
    });
    expect(args).not.toContain("--settings");
  });

  it("registers async hooks on the argv when hookNotify is given", () => {
    const { args } = buildTuiLaunch(session(), { binaryPath, hookNotify });
    const json = JSON.parse(args[args.indexOf("--settings") + 1]);
    expect(Object.keys(json.hooks).sort()).toEqual(
      [
        "Notification",
        "SessionEnd",
        "Stop",
        "StopFailure",
        "UserPromptSubmit",
      ].sort(),
    );
    for (const groups of Object.values<any>(json.hooks)) {
      expect(groups[0].hooks[0]).toMatchObject({
        type: "command",
        async: true,
      });
    }
  });

  it("quotes paths that contain spaces", () => {
    const json = JSON.parse(tuiHookSettings(hookNotify));
    const command = json.hooks.Stop[0].hooks[0].command;
    expect(command).toBe(
      "/Applications/Monoptah.app/Contents/MacOS/monocode tui-hook --spool '/Users/me/Library/Application Support/x/tui-hooks'",
    );
    const spaced = JSON.parse(
      tuiHookSettings({ ...hookNotify, cliPath: "/My Apps/monocode" }),
    );
    expect(spaced.hooks.Stop[0].hooks[0].command).toContain(
      "'/My Apps/monocode' tui-hook",
    );
  });

  it("keeps the resume id and trailing prompt in place", () => {
    const fresh = buildTuiLaunch(session(), {
      binaryPath,
      hookNotify,
      initialPrompt: "go",
    });
    expect(fresh.args.at(-1)).toBe("go");
    expect(fresh.args.at(-2)).toBe(fresh.providerSessionId);
    const resumed = buildTuiLaunch(session({ providerSessionId: "p1" }), {
      binaryPath,
      hookNotify,
    });
    expect(resumed.args.at(-1)).toBe("p1");
    expect(resumed.args).toContain("--settings");
  });

  it("never injects hooks for other providers", () => {
    for (const harness of HARNESSES.filter((id) => id !== "claude")) {
      const { args } = buildTuiLaunch(session({ harness }), {
        binaryPath,
        hookNotify,
      });
      expect(args).not.toContain("--settings");
    }
  });
});
