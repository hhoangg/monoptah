// @vitest-environment happy-dom
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { MailFolderList, groupMailboxesByDomain } from "./MailFolderList";
import { domainOf } from "../model/mailDomain";
import { MAIL_PREF_KEYS } from "../model/mailPrefs";
import type {
  MailStatus,
  MailboxStatus,
  SourceStatus,
} from "../model/mailTypes";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => undefined),
}));

const mailbox = (
  id: number,
  accountId: number,
  address: string,
  unread = 0,
): MailboxStatus => ({
  id,
  accountId,
  address,
  domain: domainOf(address),
  s3Prefix: address.split("@")[0],
  displayName: "",
  color: null,
  sortOrder: id,
  isSendable: true,
  unread,
  sync: null,
});

const source = (
  id: number,
  label: string,
  mailboxes: MailboxStatus[],
  lastError: string | null = null,
): SourceStatus =>
  ({
    id,
    label,
    enabled: true,
    sortOrder: id,
    mailboxes,
    unread: mailboxes.reduce((n, m) => n + m.unread, 0),
    lastError,
  }) as unknown as SourceStatus;

// hhoangg.com is served by two sources (different regions); byptah.com fails.
const sources = [
  source(1, "Main", [
    mailbox(1, 1, "owner@hhoangg.com", 5),
    mailbox(2, 1, "seller@hhoangg.com", 2),
  ]),
  source(2, "EU", [mailbox(3, 2, "support@hhoangg.com", 1)]),
  source(
    3,
    "Billing",
    [mailbox(4, 3, "owner@byptah.com", 4)],
    "S3 list failed",
  ),
];
const status = {
  sources,
  unreadTotal: 12,
  syncIntervalSecs: 3600,
} as MailStatus;

it("derives the domain from the address, lowercased", () => {
  expect(domainOf("Owner@HHoangg.COM")).toBe("hhoangg.com");
});

it("groups by domain: one group for a domain served by two sources", () => {
  const groups = groupMailboxesByDomain(sources);
  expect(groups.map((g) => g.domain)).toEqual(["hhoangg.com", "byptah.com"]);
  const shared = groups[0];
  expect(shared.mailboxes.map((m) => m.mailbox.id)).toEqual([1, 2, 3]);
  expect(shared.multiSource).toBe(true);
  expect(shared.unread).toBe(8);
  expect(groups[1].multiSource).toBe(false);
});

it("puts a source error on its own domain group only", () => {
  const groups = groupMailboxesByDomain(sources);
  expect(groups[0].errors).toEqual([]);
  expect(groups[1].errors).toEqual([
    { source: "Billing", message: "S3 list failed" },
  ]);
});

it("skips disabled sources", () => {
  const disabled = { ...sources[2], enabled: false } as SourceStatus;
  const groups = groupMailboxesByDomain([sources[0], disabled]);
  expect(groups.map((g) => g.domain)).toEqual(["hhoangg.com"]);
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

const render = () =>
  act(() =>
    root.render(
      createElement(MailFolderList, {
        status,
        mailboxId: null,
        folderId: null,
        onSelectAll: vi.fn(),
        onSelectMailbox: vi.fn(),
        onSelectFolder: vi.fn(),
      }),
    ),
  );

const group = (domain: string) =>
  container.querySelector<HTMLElement>(`[data-mail-domain="${domain}"]`)!;

it("renders one group per domain with the error badge on the failing one", () => {
  render();
  expect(container.querySelectorAll("[data-mail-domain]").length).toBe(2);
  expect(group("hhoangg.com").querySelector("[aria-label='Sync error']")).toBe(
    null,
  );
  expect(
    group("byptah.com")
      .querySelector("[aria-label='Sync error']")
      ?.getAttribute("title"),
  ).toBe("Billing: S3 list failed");
  // The shared domain names the source on each mailbox row.
  expect(group("hhoangg.com").textContent).toContain("EU");
  expect(group("byptah.com").textContent).not.toContain("Billing");
});

it("collapses a group and persists it to the localStorage key", () => {
  render();
  expect(group("byptah.com").querySelectorAll("button").length).toBe(2);
  act(() => group("byptah.com").querySelector("button")!.click());
  expect(group("byptah.com").querySelectorAll("button").length).toBe(1);
  expect(
    JSON.parse(localStorage.getItem(MAIL_PREF_KEYS.collapsedDomains)!),
  ).toEqual(["byptah.com"]);
  expect(MAIL_PREF_KEYS.collapsedDomains).toBe(
    "monocode.mail.collapsedDomains",
  );
});

it("starts collapsed for a domain already saved as collapsed", () => {
  localStorage.setItem(
    MAIL_PREF_KEYS.collapsedDomains,
    JSON.stringify(["hhoangg.com"]),
  );
  render();
  expect(group("hhoangg.com").querySelectorAll("button").length).toBe(1);
  expect(group("byptah.com").querySelectorAll("button").length).toBe(2);
});

it("lists smart folders and selects one by its numeric id", () => {
  const onSelectFolder = vi.fn();
  act(() =>
    root.render(
      createElement(MailFolderList, {
        status,
        mailboxId: null,
        folderId: 7,
        smartFolders: [
          { id: 7, name: "Buyers", unread: 3, color: null },
          { id: 8, name: "Invoices", unread: 0, color: "hsl(12 80% 58%)" },
        ],
        onSelectAll: vi.fn(),
        onSelectMailbox: vi.fn(),
        onSelectFolder,
      }),
    ),
  );
  const buttons = [...container.querySelectorAll("button")];
  const buyers = buttons.find((b) => b.textContent?.includes("Buyers"))!;
  const invoices = buttons.find((b) => b.textContent?.includes("Invoices"))!;
  expect(buyers.getAttribute("aria-current")).toBe("true");
  expect(buyers.textContent).toContain("3");
  expect(invoices.getAttribute("aria-current")).toBeNull();
  act(() => invoices.click());
  expect(onSelectFolder).toHaveBeenCalledWith(8);
  // Selecting a folder leaves "All mail" unselected.
  const all = buttons.find((b) => b.textContent?.includes("All mail"))!;
  expect(all.getAttribute("aria-current")).toBeNull();
});

it("says folders are loading, then empty, then shows an error", () => {
  const props = {
    status,
    mailboxId: null,
    folderId: null,
    onSelectAll: vi.fn(),
    onSelectMailbox: vi.fn(),
    onSelectFolder: vi.fn(),
  };
  act(() =>
    root.render(
      createElement(MailFolderList, { ...props, smartFolders: null }),
    ),
  );
  expect(container.textContent).toContain("Loading");
  act(() =>
    root.render(createElement(MailFolderList, { ...props, smartFolders: [] })),
  );
  expect(container.textContent).toContain("No smart folders yet.");
  act(() =>
    root.render(
      createElement(MailFolderList, {
        ...props,
        smartFolders: null,
        foldersError: "boom",
      }),
    ),
  );
  expect(container.querySelector("[role='alert']")?.textContent).toContain(
    "boom",
  );
});

it("offers the Archive view without a badge, and All mail is not selected in it", () => {
  const onSelectArchive = vi.fn();
  const props = {
    status,
    mailboxId: null,
    folderId: null,
    onSelectAll: vi.fn(),
    onSelectMailbox: vi.fn(),
    onSelectFolder: vi.fn(),
    onSelectArchive,
  };
  act(() => root.render(createElement(MailFolderList, props)));
  const archive = [...container.querySelectorAll("button")].find((b) =>
    b.textContent?.startsWith("Archive"),
  )!;
  expect(archive.textContent).toBe("Archive");
  expect(archive.querySelector("span.rounded-full")).toBeNull();
  act(() => archive.click());
  expect(onSelectArchive).toHaveBeenCalledOnce();
  act(() =>
    root.render(
      createElement(MailFolderList, { ...props, archiveSelected: true }),
    ),
  );
  const current = [...container.querySelectorAll("[aria-current='true']")];
  expect(current).toHaveLength(1);
  expect(current[0].textContent?.startsWith("Archive")).toBe(true);
});
