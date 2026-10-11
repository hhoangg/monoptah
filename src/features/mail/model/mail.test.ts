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
    [
      () =>
        mail.mailCredentialUpsert({
          label: "l",
          accessKeyId: "a",
          secretAccessKey: "s",
          defaultRegion: null,
        }),
      "mail_credential_upsert",
      {
        input: {
          label: "l",
          accessKeyId: "a",
          secretAccessKey: "s",
          defaultRegion: null,
        },
      },
    ],
    [
      () => mail.mailSourceUpsert({ id: null } as never),
      "mail_source_upsert",
      { source: { id: null } },
    ],
    [
      () => mail.mailThreads({ scope: { kind: "all" } }),
      "mail_threads",
      { query: { scope: { kind: "all" } } },
    ],
    [() => mail.mailMessageRaw(7), "mail_message_raw", { messageId: 7 }],
    [
      () => mail.mailAttachmentBytes(8),
      "mail_attachment_bytes",
      { attachmentId: 8 },
    ],
    [
      () => mail.mailOutboxSave({ kind: "new" } as never),
      "mail_outbox_save",
      { draft: { kind: "new" } },
    ],
    [() => mail.mailOutboxSend(5), "mail_outbox_send", { id: 5 }],
    [() => mail.mailOutboxRearm(5), "mail_outbox_rearm", { id: 5 }],
    [() => mail.mailOutboxCheck(5), "mail_outbox_check", { id: 5 }],
    [() => mail.mailOutboxDelete(5), "mail_outbox_delete", { id: 5 }],
    [() => mail.mailSourceTest(1), "mail_source_test", { id: 1 }],
    [
      () => mail.mailSourceDetectPrefixes(1),
      "mail_source_detect_prefixes",
      { id: 1 },
    ],
    [() => mail.mailMailboxesDelete(2), "mail_mailboxes_delete", { id: 2 }],
    [() => mail.mailSourceDelete(2), "mail_source_delete", { id: 2 }],
    [() => mail.mailIdentities(3), "mail_identities", { accountId: 3 }],
    [() => mail.mailFolderDelete(4), "mail_folder_delete", { id: 4 }],
    [
      () => mail.mailFolderPreview([], "any"),
      "mail_folder_preview",
      { rules: [], matchMode: "any" },
    ],
    [
      () => mail.mailFolderUpsert({ id: null } as never, []),
      "mail_folder_upsert",
      { folder: { id: null }, rules: [] },
    ],
  ])("passes the argument names Rust expects", async (call, command, args) => {
    await call();
    expect(invoke).toHaveBeenCalledWith(command, args);
  });
});

it("lists folders without arguments", async () => {
  await mail.mailFolders();
  expect(invoke).toHaveBeenCalledWith("mail_folders");
});

it("lists the outbox and the From picker without arguments", async () => {
  await mail.mailOutboxList();
  await mail.mailSendableMailboxes();
  expect(invoke).toHaveBeenCalledWith("mail_outbox_list");
  expect(invoke).toHaveBeenCalledWith("mail_sendable_mailboxes");
});
