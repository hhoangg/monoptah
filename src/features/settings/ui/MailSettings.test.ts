// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { MailSettings } from "./MailSettings";
import type {
  MailCredential,
  SourceRow,
  SourceStatus,
} from "../../mail/model/mailTypes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const cred = (id: number, label: string): MailCredential => ({
  id,
  provider: "aws",
  label,
  accessKeyMasked: `AKIA••••${id}XYZ`,
  defaultRegion: null,
  createdAt: 1_700_000_000_000,
  updatedAt: 1_700_000_000_000,
});

const source = (
  id: number,
  label: string,
  bucket: string,
  credentialId: number,
  s3CredentialId: number | null = null,
): SourceRow => ({
  id,
  provider: "aws",
  credentialId,
  s3CredentialId,
  label,
  sesRegion: "us-east-1",
  bucket,
  s3Region: "us-east-1",
  defaultFrom: null,
  attachPolicy: "on_demand",
  imagePolicy: "ask",
  enabled: true,
  sortOrder: id,
  createdAt: 0,
  updatedAt: 0,
  identitiesRefreshedAt: null,
  mailboxes: [
    {
      id: id * 10,
      accountId: id,
      address: `owner@${bucket}.com`,
      domain: `${bucket}.com`,
      s3Prefix: "owner/",
      displayName: "owner",
      color: null,
      sortOrder: 0,
      isSendable: true,
    },
  ],
});

let credentials: MailCredential[];
let sources: SourceRow[];
let statuses: Partial<SourceStatus>[];
let identities: unknown[];
let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  localStorage.clear();
  credentials = [cred(1, "personal"), cred(2, "acme-ltd")];
  sources = [
    source(1, "byptah-mail", "byptah", 1),
    source(2, "acme-inbound", "acme", 2, 1),
  ];
  statuses = [
    { id: 1, lastError: null, lastSyncAt: Date.now() - 12 * 60_000 },
    { id: 2, lastError: "AccessDenied: s3:ListBucket denied on acme" },
  ];
  identities = [];
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (command) => {
    switch (command) {
      case "mail_credentials":
        return credentials;
      case "mail_sources":
        return sources;
      case "mail_identities":
        return identities;
      case "mail_status":
        return { sources: statuses, unreadTotal: 0, syncIntervalSecs: 3600 };
      case "mail_sync_interval_set":
        return undefined;
      default:
        throw new Error(`Unexpected command: ${String(command)}`);
    }
  });
});
afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  localStorage.clear();
  vi.unstubAllGlobals();
});

const render = () => act(async () => root.render(createElement(MailSettings)));

function setValue(field: HTMLInputElement, value: string) {
  return act(async () => {
    Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )!.set!.call(field, value);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

const byLabel = <T extends HTMLElement>(label: string) =>
  container.querySelector<T>(`[aria-label="${label}"]`)!;

it("shows how many sources use a credential and surfaces the server's refusal on delete", async () => {
  await render();
  const row = (id: number) =>
    container.querySelector<HTMLElement>(`[data-credential-id="${id}"]`)!;
  // Credential 1 is the SES key of one source and the S3 key of another.
  expect(row(1).textContent).toContain("2 sources");
  expect(row(2).textContent).toContain("1 source");
  expect(row(1).textContent).toContain("AKIA••••1XYZ");

  const message = "In use by byptah-mail, acme-inbound. Detach it first.";
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "mail_credential_delete") throw message;
    return [];
  });
  await act(async () => byLabel("Delete credential personal").click());
  // First click only asks: AWS never re-shows a secret.
  expect(invoke).not.toHaveBeenCalledWith("mail_credential_delete", { id: 1 });
  await act(async () =>
    [...container.querySelectorAll("button")]
      .find((b) => b.textContent === "Delete key")!
      .click(),
  );
  const alerts = [...container.querySelectorAll('[role="alert"]')].map(
    (node) => node.textContent,
  );
  expect(alerts).toContain(message);
});

it("renders the error row only for the source that failed", async () => {
  await render();
  const card = (id: number) =>
    container.querySelector<HTMLElement>(`[data-source-id="${id}"]`)!;
  expect(card(1).querySelector('[data-testid="source-error"]')).toBeNull();
  const failed = card(2).querySelector('[data-testid="source-error"]');
  expect(failed?.textContent).toContain("AccessDenied");
  expect(failed?.textContent).toContain("arn:aws:s3:::acme");
});

it("shows each source's own bucket ARN in its IAM policy", async () => {
  await render();
  const policy = (id: number) =>
    container.querySelector(`[data-source-id="${id}"] pre`)!.textContent!;
  expect(policy(1)).toContain("arn:aws:s3:::byptah/*");
  expect(policy(1)).not.toContain("acme");
  expect(policy(2)).toContain("arn:aws:s3:::acme");
});

it("never fills the secret from server data and clears it after saving", async () => {
  await render();
  const secretOf = () => byLabel<HTMLInputElement>("AWS secret access key");
  expect(secretOf().type).toBe("password");
  expect(secretOf().value).toBe("");

  // Rotating opens a second editor for an existing credential: still empty.
  await act(async () =>
    [...container.querySelectorAll("button")]
      .find((b) => b.textContent === "Rotate")!
      .click(),
  );
  for (const field of container.querySelectorAll<HTMLInputElement>(
    '[aria-label="AWS secret access key"]',
  )) {
    expect(field.value).toBe("");
  }

  vi.mocked(invoke).mockImplementation(async (command) =>
    command === "mail_credential_upsert" ? cred(3, "new") : [],
  );
  // The add editor stays mounted after a save, unlike the rotate one.
  const editor = container
    .querySelector('input[aria-label="Credential label"]:not([readonly])')!
    .closest("form")!;
  const secret = editor.querySelector<HTMLInputElement>(
    '[aria-label="AWS secret access key"]',
  )!;
  await setValue(editor.querySelector('[aria-label="Credential label"]')!, "x");
  await setValue(
    editor.querySelector('[aria-label="AWS access key id"]')!,
    "AKIAX",
  );
  await setValue(secret, "hunter2");
  expect(secret.value).toBe("hunter2");
  await act(async () => {
    editor.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
  });
  expect(secret.value).toBe("");
  expect(container.textContent).not.toContain("hunter2");
});

it("pushes the interval to Rust when it changes", async () => {
  await render();
  await act(async () =>
    container
      .querySelector<HTMLButtonElement>('[aria-label^="Sync interval"]')!
      .click(),
  );
  const option = [
    ...document.querySelectorAll<HTMLElement>('[role="option"]'),
  ].find((node) => node.textContent?.includes("Every 5 minutes"))!;
  await act(async () => option.click());
  expect(invoke).toHaveBeenCalledWith("mail_sync_interval_set", { secs: 300 });
});

it("explains sending_disabled as an account setting, not a verification failure", async () => {
  identities = [
    {
      accountId: 1,
      identity: "byptah.com",
      kind: "domain",
      status: "sending_disabled",
    },
  ];
  await render();
  const text = container.querySelector('[data-source-id="1"]')!.textContent!;
  expect(text).toContain("not a verification problem");
  expect(text).not.toContain("could not verify");
});

it("keeps the secret when a save is rejected", async () => {
  await render();
  const form = container
    .querySelector('input[aria-label="Credential label"]:not([readonly])')!
    .closest("form")!;
  const secret = form.querySelector<HTMLInputElement>(
    '[aria-label="AWS secret access key"]',
  )!;
  await setValue(form.querySelector('[aria-label="Credential label"]')!, "x");
  await setValue(
    form.querySelector('[aria-label="AWS access key id"]')!,
    "bad",
  );
  await setValue(secret, "hunter2");
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "mail_credential_upsert") throw "Not an access key id.";
    return [];
  });
  await act(async () => {
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
  });
  expect(form.textContent).toContain("Not an access key id.");
  expect(secret.value).toBe("hunter2");
});

it("refetches after Test so identities and sendable pills update", async () => {
  vi.mocked(invoke).mockImplementation(async (command) => {
    if (command === "mail_source_test")
      return {
        accountId: 1,
        label: "byptah-mail",
        s3: { ok: true, action: "s3:ListBucket", kind: null, message: null },
        sesIdentities: {
          ok: true,
          action: "ses:ListEmailIdentities",
          kind: null,
          message: null,
        },
        sesAccount: {
          ok: true,
          action: "ses:GetAccount",
          kind: null,
          message: null,
        },
        identityCount: 1,
        sandbox: false,
      };
    if (command === "mail_credentials") return credentials;
    if (command === "mail_sources") return sources;
    if (command === "mail_identities") return identities;
    return { sources: statuses, unreadTotal: 0, syncIntervalSecs: 3600 };
  });
  await render();
  const calls = () =>
    vi.mocked(invoke).mock.calls.filter(([c]) => c === "mail_identities")
      .length;
  const before = calls();
  await act(async () =>
    [...container.querySelectorAll('[data-source-id="1"] button')]
      .find((b) => b.textContent === "Test")!
      .click(),
  );
  expect(calls()).toBe(before + 1);
});

it("does not blame verification on a receive-only mailbox whose identity has sending disabled", async () => {
  sources[0].mailboxes[0].isSendable = false;
  identities = [
    {
      accountId: 1,
      identity: "byptah.com",
      kind: "domain",
      status: "sending_disabled",
    },
  ];
  await render();
  const pill = [
    ...container.querySelectorAll<HTMLElement>(
      '[data-source-id="1"] span[title]',
    ),
  ].find((n) => n.textContent === "receive only")!;
  expect(pill.title).toContain("not a verification problem");
  expect(pill.title).not.toContain("no verified SES identity");
});

it("keeps colour and display name when an existing address is re-added", async () => {
  sources[0].mailboxes[0].color = "#abc";
  sources[0].mailboxes[0].displayName = "Boss";
  sources[0].mailboxes[0].sortOrder = 7;
  await render();
  vi.mocked(invoke).mockImplementation(async () => []);
  await setValue(
    byLabel<HTMLInputElement>("New mailbox address for byptah-mail"),
    "owner@byptah.com",
  );
  await setValue(
    byLabel<HTMLInputElement>("New mailbox S3 prefix for byptah-mail"),
    "owner2/",
  );
  const form = byLabel("New mailbox address for byptah-mail").closest("form")!;
  await act(async () => {
    form.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
  });
  expect(invoke).toHaveBeenCalledWith("mail_mailboxes_upsert", {
    accountId: 1,
    rows: [
      expect.objectContaining({
        displayName: "Boss",
        color: "#abc",
        s3Prefix: "owner2/",
        sortOrder: 7,
      }),
    ],
  });
});

it("offers regions as a dropdown that keeps an unknown stored region", async () => {
  sources[0].sesRegion = "xx-custom-1";
  await render();
  await act(async () =>
    [...container.querySelectorAll('[data-source-id="1"] button')]
      .find((b) => b.textContent === "Edit")!
      .click(),
  );
  const trigger = container.querySelector<HTMLElement>(
    '[aria-label^="SES region"]',
  )!;
  expect(trigger.tagName).toBe("BUTTON");
  expect(trigger.textContent).toContain("xx-custom-1");
});

it("lets a new source use a region outside the list through Custom region", async () => {
  await render();
  await act(async () =>
    [...container.querySelectorAll("button")]
      .find((b) => b.textContent === "Add source")!
      .click(),
  );
  const form = container.querySelector<HTMLFormElement>(
    'form[aria-label="Add source"]',
  )!;
  await act(async () =>
    form.querySelector<HTMLElement>('[aria-label^="S3 region"]')!.click(),
  );
  const options = [
    ...document.querySelectorAll<HTMLElement>('[role="option"]'),
  ];
  expect(options.some((o) => o.textContent === "ca-west-1")).toBe(true);
  await act(async () =>
    options.find((o) => o.textContent === "Custom region…")!.click(),
  );
  const field = form.querySelector<HTMLInputElement>(
    '[aria-label="S3 region (custom)"]',
  )!;
  await setValue(field, "us-gov-west-1");
  expect(field.value).toBe("us-gov-west-1");
});

it("writes the remote-image choice to every source", async () => {
  await render();
  const upserts: unknown[] = [];
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === "mail_source_upsert") {
      upserts.push((args as { source: unknown }).source);
      return undefined;
    }
    if (command === "mail_sources") return sources;
    if (command === "mail_credentials") return credentials;
    if (command === "mail_identities") return identities;
    if (command === "mail_status")
      return { sources: statuses, unreadTotal: 0, syncIntervalSecs: 3600 };
    return undefined;
  });
  await act(async () =>
    container
      .querySelector<HTMLElement>('[aria-label^="Load remote images:"]')!
      .click(),
  );
  await act(async () =>
    [...document.querySelectorAll('[role="option"]')]
      .find((b) => b.textContent?.includes("Never load"))!
      .dispatchEvent(new MouseEvent("click", { bubbles: true })),
  );
  expect(upserts).toHaveLength(2);
  expect(
    upserts.map((u) => (u as { imagePolicy: string }).imagePolicy),
  ).toEqual(["never", "never"]);
  expect(container.textContent).not.toContain("never sees your IP");
});

it("says the image policy is set per source when sources differ", async () => {
  sources = [{ ...sources[0], imagePolicy: "never" }, sources[1]];
  await render();
  expect(
    container.querySelector(
      '[aria-label="Load remote images: Set per source"]',
    ),
  ).not.toBeNull();
});
