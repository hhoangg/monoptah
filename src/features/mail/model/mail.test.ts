import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as mail from "./mail";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => vi.mocked(invoke).mockReset());

// Tauri matches argument names exactly (camelCase), so a typo fails at runtime only.
describe("mail command wrappers", () => {
  it.each([
    [() => mail.mailCredentialDelete(3), "mail_credential_delete", { id: 3 }],
    [
      () => mail.mailMailboxesUpsert(2, []),
      "mail_mailboxes_upsert",
      { accountId: 2, rows: [] },
    ],
    [
      () => mail.mailSyncNow({ kind: "mailbox", id: 4 }),
      "mail_sync_now",
      { scope: { kind: "mailbox", id: 4 } },
    ],
    [
      () => mail.mailSyncIntervalSet(900),
      "mail_sync_interval_set",
      { secs: 900 },
    ],
    [() => mail.mailIdentities(), "mail_identities", { accountId: null }],
    [() => mail.mailThread(9), "mail_thread", { threadId: 9 }],
    [
      () => mail.mailMark([1, 2], true),
      "mail_mark",
      { threadIds: [1, 2], read: true },
    ],
    [
      () => mail.mailStar([1], false),
      "mail_star",
      { threadIds: [1], starred: false },
    ],
    [
      () => mail.mailAttachmentOpen(5),
      "mail_attachment_open",
      { attachmentId: 5 },
    ],
    [
      () => mail.mailRemoteImage(6, "https://x/y.png"),
      "mail_remote_image",
      { messageId: 6, url: "https://x/y.png" },
    ],
  ])("passes the argument names Rust expects", async (call, command, args) => {
    await call();
    expect(invoke).toHaveBeenCalledWith(command, args);
  });
});
