import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import {
  buildQuote,
  FORWARD_WORDS,
  REPLY_WORDS,
  defaultFrom,
  parseRecipients,
  prefixedSubject,
  recipientsFor,
  senderGroups,
  senderOk,
  sendMode,
} from "./mailReply";
import type {
  IdentityRow,
  MailMailbox,
  MessageRow,
  SendableMailbox,
  SourceStatus,
} from "./mailTypes";

const msg = (over: Partial<MessageRow> = {}): MessageRow =>
  ({
    id: 1,
    direction: "inbound",
    subject: "Order",
    fromName: "Ann",
    fromAddr: "ann@example.org",
    replyToAddr: null,
    to: [{ name: null, addr: "owner@byptah.com" }],
    cc: [],
    bcc: [],
    dateUtc: 1_700_000_000,
    snippet: "snip",
    bodyText: "line one\n\nline two",
    bodyHtml: "<p>line one</p>",
    ...over,
  }) as MessageRow;

describe("recipientsFor", () => {
  const own = ["owner@byptah.com", "Support@Byptah.com"];

  it("reply-all leaves out the sending identity and every own mailbox", () => {
    const m = msg({
      to: [
        { name: null, addr: "OWNER@byptah.com" },
        { name: null, addr: "bob@example.org" },
      ],
      cc: [
        { name: null, addr: "support@byptah.com" },
        { name: null, addr: "cy@example.org" },
        { name: null, addr: "bob@example.org" },
      ],
    });
    const r = recipientsFor("reply_all", m, own);
    expect(r.to.map((a) => a.addr)).toEqual([
      "ann@example.org",
      "bob@example.org",
    ]);
    // bob is already in To, so he is not copied as well.
    expect(r.cc.map((a) => a.addr)).toEqual(["cy@example.org"]);
  });

  it("reply goes to Reply-To, and only there", () => {
    const m = msg({
      replyToAddr: "list@example.org",
      cc: [{ name: null, addr: "x@y.org" }],
    });
    expect(recipientsFor("reply", m, own)).toEqual({
      to: [{ name: null, addr: "list@example.org" }],
      cc: [],
    });
  });

  it("answering a message we sent goes to whoever we sent it to", () => {
    const m = msg({
      direction: "outbound",
      fromAddr: "owner@byptah.com",
      to: [{ name: null, addr: "ann@example.org" }],
    });
    expect(recipientsFor("reply_all", m, own).to.map((a) => a.addr)).toEqual([
      "ann@example.org",
    ]);
  });

  it("forward and new start with nobody", () => {
    expect(recipientsFor("forward", msg(), own)).toEqual({ to: [], cc: [] });
    expect(recipientsFor("new", msg(), own)).toEqual({ to: [], cc: [] });
  });
});

describe("prefixedSubject", () => {
  it("does not double a prefix on a reply to a reply", () => {
    expect(prefixedSubject("Order", "reply")).toBe("Re: Order");
    expect(prefixedSubject("Re: Order", "reply")).toBe("Re: Order");
    expect(prefixedSubject("RE: Order", "reply_all")).toBe("RE: Order");
    expect(prefixedSubject("  re:Order", "reply")).toBe("re:Order");
  });

  it("forwards get Fwd: once, and a reply to a forward still gets Re:", () => {
    expect(prefixedSubject("Order", "forward")).toBe("Fwd: Order");
    expect(prefixedSubject("Fwd: Order", "forward")).toBe("Fwd: Order");
    expect(prefixedSubject("FW: Order", "forward")).toBe("FW: Order");
    expect(prefixedSubject("Fwd: Order", "reply")).toBe("Re: Fwd: Order");
  });

  it("recognises the other languages and counted forms thread grouping strips", () => {
    expect(prefixedSubject("AW: Order", "reply")).toBe("AW: Order");
    expect(prefixedSubject("Re[2]: Order", "reply_all")).toBe("Re[2]: Order");
    expect(prefixedSubject("Trả lời: Order", "reply")).toBe("Trả lời: Order");
    // Decomposed diacritics (NFD) are the same word.
    expect(prefixedSubject("Tra\u0309 lo\u031b\u0300i: x", "reply")).toBe(
      "Trả lời: x".normalize("NFC"),
    );
    expect(prefixedSubject("Chuyển tiếp: Order", "forward")).toBe(
      "Chuyển tiếp: Order",
    );
    expect(prefixedSubject("Fw: Order", "forward")).toBe("Fw: Order");
  });

  it("keeps its prefix words in step with mail_thread.rs", () => {
    const rust = readFileSync(
      resolve(__dirname, "../../../../src-tauri/src/mail_thread.rs"),
      "utf8",
    );
    const block = /REPLY_PREFIXES: \[&str; \d+\] = \[([^\]]*)\]/.exec(rust);
    expect(block).not.toBeNull();
    const words = [...block![1].matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    expect(words.length).toBeGreaterThan(5);
    expect([...REPLY_WORDS, ...FORWARD_WORDS].sort()).toEqual(
      [...words].sort(),
    );
  });

  it("leaves a new message's subject alone", () => {
    expect(prefixedSubject("Hi", "new")).toBe("Hi");
  });
});

describe("parseRecipients", () => {
  it("reads names, bare addresses and reports the rest apart", () => {
    expect(parseRecipients('"Ann B" <ann@x.org>, bob@y.org; nonsense')).toEqual(
      {
        valid: [
          { name: "Ann B", addr: "ann@x.org" },
          { name: null, addr: "bob@y.org" },
        ],
        invalid: ["nonsense"],
      },
    );
  });
});

const mailbox = (id: number, address: string, accountId: number): MailMailbox =>
  ({
    id,
    accountId,
    address,
    domain: address.split("@")[1],
    s3Prefix: `${address}/`,
    displayName: address,
    color: null,
    sortOrder: id,
    isSendable: true,
  }) as MailMailbox;

const source = (
  id: number,
  label: string,
  addresses: string[],
  over: Partial<SourceStatus> = {},
): SourceStatus =>
  ({
    id,
    label,
    sesRegion: "us-east-1",
    enabled: true,
    sortOrder: id,
    lastError: null,
    defaultFrom: null,
    mailboxes: addresses.map((a, i) => mailbox(id * 10 + i, a, id)),
    ...over,
  }) as unknown as SourceStatus;

const sendable = (address: string, accountId: number, isDefault = false) =>
  ({ ...mailbox(1, address, accountId), isDefault }) as SendableMailbox;

describe("senderGroups", () => {
  const sources = [
    source(1, "byptah-mail", ["owner@byptah.com", "support@byptah.com"]),
    source(2, "acme-inbound", ["hello@acme.com"], {
      sesRegion: "eu-west-1",
      lastError: "AccessDenied",
    }),
  ];
  const ids: IdentityRow[] = [
    {
      accountId: 1,
      identity: "byptah.com",
      kind: "domain",
      status: "verified",
    },
    {
      accountId: 1,
      identity: "support@byptah.com",
      kind: "email",
      status: "sending_disabled",
    },
  ];
  const groups = senderGroups(
    sources,
    [sendable("owner@byptah.com", 1, true), sendable("hello@acme.com", 2)],
    ids,
  );

  it("groups by domain and names the source behind each", () => {
    expect(groups.map((g) => g.domain)).toEqual(["byptah.com", "acme.com"]);
    expect(groups[0].sources).toEqual(["byptah-mail · us-east-1"]);
    expect(groups[1].sources).toEqual(["acme-inbound · eu-west-1"]);
  });

  it("greys what cannot send, saying why in the right words", () => {
    const by = Object.fromEntries(
      groups.flatMap((g) => g.entries).map((e) => [e.address, e]),
    );
    expect(senderOk(by["owner@byptah.com"])).toBe(true);
    // AWS verified it and switched sending off: not a verification problem.
    expect(by["support@byptah.com"].problem).toBe("sendingDisabled");
    expect(by["hello@acme.com"].problem).toBe("sourceFailing");
  });

  it("says not verified when no identity covers the address", () => {
    const g = senderGroups([source(1, "s", ["a@x.com"])], [], []);
    expect(g[0].entries[0].problem).toBe("notVerified");
  });

  it("defaults to the mailbox the message arrived in, else the default From", () => {
    const m = msg({
      to: [
        { name: null, addr: "hello@acme.com" },
        { name: null, addr: "owner@byptah.com" },
      ],
    });
    // acme is greyed (source failing), so the next addressed mailbox wins.
    expect(defaultFrom(groups, m, null)).toBe("owner@byptah.com");
    expect(defaultFrom(groups, null, null)).toBe("owner@byptah.com");
  });
});

describe("sendMode", () => {
  it("offers neither Send nor Retry on an unconfirmed row", () => {
    expect(sendMode("unconfirmed")).toEqual({ kind: "checkFirst" });
  });

  it("offers Send again only on a failed row, where nothing was delivered", () => {
    expect(sendMode("failed")).toEqual({ kind: "send", label: "Send again" });
    expect(sendMode(null)).toEqual({ kind: "send", label: "Send" });
    expect(sendMode("sending").kind).toBe("busy");
    expect(sendMode("sent").kind).toBe("done");
  });
});

describe("buildQuote", () => {
  it("escapes sender-controlled text in the attribution and keeps the sanitised body as is", () => {
    const q = buildQuote(
      "reply",
      msg({ fromName: "<b>Eve</b>" }),
      () => "DATE",
    )!;
    expect(q.html).toContain("On DATE, &lt;b&gt;Eve&lt;/b&gt; wrote:");
    expect(q.html).toContain("<p>line one</p>");
    expect(q.text).toBe(
      "On DATE, <b>Eve</b> wrote:\n> line one\n>\n> line two",
    );
  });

  it("is absent for a new message", () => {
    expect(buildQuote("new", msg())).toBeNull();
  });

  it("a forward carries the original's headers, not a > quote", () => {
    const q = buildQuote("forward", msg(), () => "DATE")!;
    expect(q.text).toContain("Forwarded message");
    expect(q.text).toContain("Subject: Order");
    expect(q.text).not.toContain("> ");
  });
});
