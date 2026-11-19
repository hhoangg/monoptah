import { beforeEach, describe, expect, it } from "vitest";
import { forgetChatDrafts, readChatDraft, resetChatDrafts, writeChatDraft } from "./chatDraft";

beforeEach(() => resetChatDrafts());

describe("chat drafts", () => {
  it("keeps the text of a chat per desktop and chat, and an empty text drops it", () => {
    writeChatDraft("d1", "s1", "hello");
    writeChatDraft("d1", "mono:m1", "to the mono");
    expect(readChatDraft("d1", "s1")).toBe("hello");
    expect(readChatDraft("d1", "mono:m1")).toBe("to the mono");
    expect(readChatDraft("d2", "s1")).toBe("");
    writeChatDraft("d1", "s1", "");
    expect(readChatDraft("d1", "s1")).toBe("");
  });

  it("is dropped for a forgotten desktop and by a reset, and never stored on disk", () => {
    writeChatDraft("d1", "s1", "a");
    writeChatDraft("d2", "s1", "unique-draft-text");
    forgetChatDrafts("d1");
    expect(readChatDraft("d1", "s1")).toBe("");
    expect(readChatDraft("d2", "s1")).toBe("unique-draft-text");
    expect(JSON.stringify({ ...localStorage, ...sessionStorage })).not.toContain("unique-draft-text");
    resetChatDrafts();
    expect(readChatDraft("d2", "s1")).toBe("");
  });
});
