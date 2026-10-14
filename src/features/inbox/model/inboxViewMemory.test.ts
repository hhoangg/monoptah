import { beforeEach, describe, expect, it } from "vitest";
import {
  getRememberedInboxView,
  resetRememberedInboxViews,
  setRememberedInboxView,
} from "./inboxViewMemory";

describe("remembered inbox view", () => {
  beforeEach(() => resetRememberedInboxViews());

  it("starts every source with an empty view", () => {
    expect(getRememberedInboxView("github")).toEqual({
      search: "",
      selectedKey: null,
      scrollTop: 0,
      listLimit: null,
    });
  });

  it("keeps the view of each source apart", () => {
    setRememberedInboxView("github", {
      search: "fix",
      selectedKey: "github:acme/web#pr:7",
      scrollTop: 420,
      listLimit: 64,
    });
    setRememberedInboxView("linear", {
      search: "",
      selectedKey: "linear:ENG-1",
      scrollTop: 0,
      listLimit: 32,
    });

    expect(getRememberedInboxView("github")).toEqual({
      search: "fix",
      selectedKey: "github:acme/web#pr:7",
      scrollTop: 420,
      listLimit: 64,
    });
    expect(getRememberedInboxView("linear").selectedKey).toBe("linear:ENG-1");
    expect(getRememberedInboxView("jira").selectedKey).toBeNull();
  });

  it("returns a copy so callers cannot mutate the store", () => {
    setRememberedInboxView("github", {
      search: "a",
      selectedKey: null,
      scrollTop: 1,
      listLimit: null,
    });
    const view = getRememberedInboxView("github");
    view.search = "changed";
    expect(getRememberedInboxView("github").search).toBe("a");
  });

  it("clamps a bad scroll offset to zero", () => {
    setRememberedInboxView("gitlab", {
      search: "",
      selectedKey: null,
      scrollTop: -20,
      listLimit: null,
    });
    expect(getRememberedInboxView("gitlab").scrollTop).toBe(0);
    setRememberedInboxView("gitlab", {
      search: "",
      selectedKey: null,
      scrollTop: Number.NaN,
      listLimit: null,
    });
    expect(getRememberedInboxView("gitlab").scrollTop).toBe(0);
  });

  it("forgets everything on reset", () => {
    setRememberedInboxView("clickup", {
      search: "x",
      selectedKey: "k",
      scrollTop: 5,
      listLimit: 32,
    });
    resetRememberedInboxViews();
    expect(getRememberedInboxView("clickup").search).toBe("");
  });
});
