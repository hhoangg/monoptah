import { describe, expect, it } from "vitest";
import { projectMenuExtraItems } from "./useProjectMenu";

function items(canForwardQuestions: boolean) {
  return projectMenuExtraItems(
    false,
    true,
    true,
    true,
    [],
    [],
    undefined,
    true,
    canForwardQuestions,
  );
}

describe("project menu Telegram item", () => {
  it("is offered for projects that can forward questions", () => {
    const item = items(true).find((entry) => entry.id === "telegram-settings");
    expect(item?.label).toBe("Telegram…");
  });

  it("is hidden for remote projects", () => {
    expect(items(false).some((entry) => entry.id === "telegram-settings")).toBe(
      false,
    );
  });
});
