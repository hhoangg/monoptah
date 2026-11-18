import { ModelCatalogSchema, SendableRuntimeModeSchema, SessionConfigureParamsSchema, SessionStartParamsSchema } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
// The protocol file is not exported by the package yet; the mirror must behave exactly like it.
import * as protocol from "../../../../protocol/src/models.ts";
import { makeCatalog } from "./fixtures";
import * as mirror from "./modelSettings";
import { buildModelSettingsParams, buildStartModelParams } from "./params";

const catalog = makeCatalog();
const [opus, sonnet, gpt] = catalog.models;

describe("mirror of protocol/src/models.ts", () => {
  it("fixture is a valid catalog", () => {
    expect(ModelCatalogSchema.safeParse(catalog).success).toBe(true);
  });

  it("every helper agrees with the protocol for every fixture model", () => {
    for (const model of catalog.models) {
      expect(mirror.modelEffortSetting(model)).toEqual(protocol.modelEffortSetting(model));
      expect(mirror.defaultModelSettings(model)).toEqual(protocol.defaultModelSettings(model));
      expect(mirror.fastSetting(model)).toEqual(protocol.fastSetting(model));
      expect(mirror.otherSettings(model)).toEqual(protocol.otherSettings(model));
      expect(mirror.effortLabel(model, { effort: "high", reasoningEffort: "low" })).toEqual(protocol.modelEffortLabel(model, { effort: "high", reasoningEffort: "low" }));
      for (const setting of model.settings) expect(mirror.settingValue({}, setting)).toEqual(protocol.settingValue({}, setting));
      for (const current of [undefined as Record<string, string> | undefined, { effort: "high" }, { effort: "extra-high" }, { effort: "nope", fast: "true" }] as Array<Record<string, string> | undefined>) {
        expect(mirror.mergeModelSettings(model, current)).toEqual(protocol.mergeModelSettings(model, current));
      }
    }
    for (const id of ["effort", "reasoning", "reasoningEffort", "thinking", "variant", "fast", "x"]) expect(mirror.isEffortSettingId(id)).toBe(protocol.isEffortSettingId(id));
    for (const raw of [null, [], "x", { a: "b", c: 1 }]) expect(mirror.parseStringRecord(raw)).toEqual(protocol.parseStringRecord(raw));
    const choice = { harness: "claude", model: "opus", modelSettings: { effort: "max" } };
    expect(mirror.withModel(choice, sonnet!)).toEqual(protocol.withModel(choice, sonnet!));
    expect(mirror.withModel(choice, gpt!)).toEqual(protocol.withModel(choice, gpt!));
  });

  it("settings carry over where they still apply", () => {
    expect(mirror.withModel({ harness: "claude", model: "opus", modelSettings: { effort: "max", fast: "true" } }, sonnet!).modelSettings).toEqual({ effort: "max" });
    expect(mirror.fastSetting(opus)?.on).toBe("true");
  });
});

describe("no code path emits full-access", () => {
  it("toEmitted drops a reported full-access and unknown modes", () => {
    const base = { harness: "claude", model: "opus", modelSettings: {} };
    expect(mirror.toEmitted({ ...base, runtimeMode: "full-access" }).runtimeMode).toBeUndefined();
    expect("runtimeMode" in mirror.toEmitted({ ...base, runtimeMode: "full-access" })).toBe(false);
    expect(mirror.toEmitted({ ...base, runtimeMode: "bogus" as never }).runtimeMode).toBeUndefined();
    expect(mirror.toEmitted({ ...base, runtimeMode: "full-access" }, "auto").runtimeMode).toBe("auto");
    expect(mirror.toEmitted({ ...base, runtimeMode: "auto-accept-edits" }).runtimeMode).toBe("auto-accept-edits");
  });

  it("the param builders never put full-access on the wire, whatever they are given", () => {
    const hostile = { harness: "claude", model: "opus", modelSettings: { effort: "max", bad: 1 as unknown as string }, runtimeMode: "full-access" as never };
    for (const params of [buildStartModelParams(hostile), buildModelSettingsParams("c1", hostile)]) {
      expect(JSON.stringify(params)).not.toContain("full-access");
      expect("runtimeMode" in params).toBe(false);
      expect(params).toMatchObject({ harness: "claude", model: "opus", modelSettings: { effort: "max" } });
    }
    expect(SessionConfigureParamsSchema.safeParse(buildModelSettingsParams("c1", hostile)).success).toBe(true);
    expect(SessionStartParamsSchema.safeParse({ projectId: "p", text: "hi", worktreePath: "/w", ...buildStartModelParams(hostile) }).success).toBe(true);
  });

  it("every sendable mode passes through and validates, and the empty case adds nothing", () => {
    for (const mode of SendableRuntimeModeSchema.options) {
      const params = buildModelSettingsParams("c1", { harness: "claude", model: "opus", modelSettings: {}, runtimeMode: mode, providerAccountId: "work" });
      expect(params).toMatchObject({ runtimeMode: mode, providerAccountId: "work" });
      expect(SessionConfigureParamsSchema.safeParse(params).success).toBe(true);
    }
    expect(buildStartModelParams(undefined)).toEqual({});
  });
});
