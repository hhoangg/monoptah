import { describe, expect, it } from "vitest";
import { currentPeriod, previousPeriod } from "../../contributors/model/period";
import type { Period } from "../../contributors/model/period";
import { buildReport, renderReportHtml, reportFileName } from "./report";
import type { ReportInput } from "./report";
import { DEFAULT_TICKET_POINTS } from "./tickets";
import type {
  Criteria,
  TicketAssessment,
  TicketAssignee,
  TicketRecord,
  WorkspaceScope,
} from "./tickets";

const NOW = new Date(2026, 9, 18, 12, 0, 0);
const OCTOBER = currentPeriod("month", NOW);
const SEPTEMBER = previousPeriod(OCTOBER) as Period;

const scope: WorkspaceScope = {
  workspaceId: "w1",
  name: "Casium",
  spaceIds: null,
  excludedStatuses: [],
  deliveredStatuses: [],
  excludedMembers: [],
};

const user = (id: string, name = `User ${id}`): TicketAssignee => ({
  id,
  name,
  email: `${id}@example.com`,
});

const uniform = (score: number): Criteria => ({
  ambiguity: score,
  investigation: score,
  scope: score,
  risk: score,
  coordination: score,
});

const assessment = (
  criteria: Criteria,
  reason = "reason",
): TicketAssessment => ({
  criteria,
  reasons: {
    ambiguity: `${reason}-ambiguity`,
    investigation: `${reason}-investigation`,
    scope: `${reason}-scope`,
    risk: `${reason}-risk`,
    coordination: `${reason}-coordination`,
  },
  rubricVersion: 1,
  model: "test",
  runner: "api",
  assessedAt: "2026-10-01T00:00:00Z",
});

/** A local-time noon date as an ISO string, away from day edges. */
const at = (y: number, m: number, d: number) =>
  new Date(y, m - 1, d, 12, 0, 0).toISOString();

function ticket(id: string, patch: Partial<TicketRecord> = {}): TicketRecord {
  return {
    source: "clickup",
    id,
    title: `Ticket ${id}`,
    url: `https://app.clickup.com/t/${id}`,
    workspaceId: "w1",
    spaceId: "s1",
    status: "closed",
    statusType: "closed",
    updatedAt: at(2026, 10, 7),
    doneAt: at(2026, 10, 7),
    statusSince: null,
    parentId: null,
    hasSubtasks: false,
    assignees: [],
    tags: [],
    assessment: null,
    overrideLevel: null,
    overrideNote: null,
    ...patch,
  };
}

function input(
  tickets: TicketRecord[],
  patch: Partial<ReportInput> = {},
): ReportInput {
  return {
    workspaceName: "Casium",
    period: OCTOBER,
    previousPeriod: SEPTEMBER,
    scope,
    points: DEFAULT_TICKET_POINTS,
    tickets,
    previousTickets: [],
    ai: { runner: "claude-code", model: "claude-sonnet-5-5", hasApiKey: false },
    generatedAt: NOW,
    ...patch,
  };
}

const tile = (data: ReturnType<typeof buildReport>, id: string) =>
  data.tiles.find((t) => t.id === id)!;

describe("reportFileName", () => {
  it("slugs the workspace and appends the period id", () => {
    expect(reportFileName("Casium Dev / EU!", OCTOBER)).toBe(
      "ticket-contributors-casium-dev-eu-2026-10.html",
    );
    expect(reportFileName("  ", currentPeriod("7d", NOW))).toBe(
      "ticket-contributors-workspace-7d.html",
    );
  });
});

describe("renderReportHtml safety", () => {
  const evil = `<script>alert(1)</script> "quoted" & 'single'`;
  const tickets = [
    ticket("a", {
      title: evil,
      url: 'https://app.clickup.com/t/a?x=1&y="2"',
      status: evil,
      assignees: [user("1", evil)],
      assessment: assessment(uniform(2), evil),
    }),
    ticket("b", {
      title: "plain http link",
      url: "http://insecure.example.com/t/b",
      assignees: [user("1", evil)],
      overrideLevel: "L",
      overrideNote: evil,
    }),
  ];

  const html = () =>
    renderReportHtml(
      buildReport(
        input(tickets, {
          workspaceName: evil,
          scope: {
            ...scope,
            deliveredStatuses: [evil],
            excludedStatuses: ["<i>excluded</i>"],
          },
          ai: { runner: "api", model: evil, hasApiKey: true },
        }),
      ),
    );

  it("escapes every ClickUp and AI string and has no script", () => {
    const out = html();
    expect(out).not.toContain("<script");
    expect(out).not.toContain("alert(1)</");
    expect(out).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
    expect(out).toContain("&quot;quoted&quot;");
    expect(out).toContain("&#39;single&#39;");
    expect(out).toContain("Excluded statuses: &lt;i&gt;excluded&lt;/i&gt;");
    expect(out).not.toContain("<i>");
  });

  it("uses no http:// and no external host besides https ticket links", () => {
    const out = html();
    expect(out).not.toContain("http://");
    const hosts = [...out.matchAll(/https:\/\/([^/"'\s]+)/g)].map((m) => m[1]);
    expect(new Set(hosts)).toEqual(new Set(["app.clickup.com"]));
    expect(out).not.toMatch(/<(link|img|iframe|object|embed)\b/i);
    expect(out).not.toContain("@import");
    expect(out).not.toMatch(/url\(/);
  });

  it("has no event handler attribute, even with a hostile URL", () => {
    const hostile = ticket("h", {
      title: 'x" onmouseover="alert(1)',
      url: 'https://a.example.com/" onmouseover="alert(2)" x="',
      overrideLevel: "XL",
      assignees: [user("1", 'n" onclick="alert(3)')],
    });
    const out = renderReportHtml(buildReport(input([...tickets, hostile])));
    expect(out).toContain("&quot; onmouseover=&quot;alert(2)");
    // Drop quoted attribute values, then look for on...= inside any tag.
    const bare = out.replace(/"[^"]*"/g, '""');
    expect(bare).not.toMatch(/<[^<>]*\son[a-z]+\s*=/i);
  });

  it("links a ticket only when its URL is https", () => {
    const data = buildReport(input(tickets));
    const secure = data.hardest.find((h) => h.id === "a")!;
    const insecure = data.hardest.find((h) => h.id === "b")!;
    expect(secure.url).toBe('https://app.clickup.com/t/a?x=1&y="2"');
    expect(insecure.url).toBeNull();
    const out = renderReportHtml(data);
    expect(out).toContain(
      'href="https://app.clickup.com/t/a?x=1&amp;y=&quot;2&quot;"',
    );
    expect(out).not.toContain("insecure.example.com");
    expect(out).toContain("plain http link");
  });
});

describe("buildReport tiles", () => {
  const tickets = [
    ticket("a", {
      assignees: [user("1"), user("2")],
      assessment: assessment(uniform(1)), // M = 16, split 8 + 8
    }),
    ticket("b", { assignees: [user("1")], overrideLevel: "XL" }), // 48
    ticket("c", { assignees: [user("2")] }), // unscored
    ticket("d", { assignees: [user("3")], assessment: assessment(uniform(0)) }), // XS = 3
    ticket("out", { doneAt: at(2026, 9, 12), assignees: [user("1")] }),
  ];

  it("match the member table sums", () => {
    const data = buildReport(input(tickets));
    const teamPoints = data.members.reduce((s, m) => s + m.points, 0);
    expect(tile(data, "points").value).toBe(String(teamPoints));
    expect(teamPoints).toBe(8 + 48 + 8 + 3);
    expect(tile(data, "delivered").value).toBe("4");
    expect(data.members.reduce((s, m) => s + m.delivered, 0)).toBe(5); // ticket a counts for two members
    expect(tile(data, "scored").value).toBe("75%");
    expect(tile(data, "scored").sub).toBe("3 of 4");
    expect(tile(data, "contributors").value).toBe(
      String(data.members.filter((m) => m.delivered > 0).length),
    );
    expect(tile(data, "median").value).toBe("8"); // points 56, 8 and 3
  });

  it("counts active tickets and members with only active work", () => {
    const data = buildReport(
      input([
        ...tickets,
        ticket("w", {
          status: "in progress",
          statusType: "custom",
          doneAt: null,
          updatedAt: at(2026, 10, 10),
          assignees: [user("9", "Only Active")],
        }),
      ]),
    );
    expect(tile(data, "active").value).toBe("1");
    expect(tile(data, "contributors").value).toBe("3");
    expect(data.load.find((m) => m.id === "9")).toMatchObject({
      delivered: 0,
      active: 1,
    });
    const html = renderReportHtml(data);
    expect(html).toContain("Only Active");
  });

  it("works with nothing delivered", () => {
    const data = buildReport(input([]));
    expect(data.notes).toEqual([]);
    expect(tile(data, "delivered").value).toBe("0");
    expect(tile(data, "scored").value).toBe("-");
    expect(tile(data, "median").value).toBe("-");
    expect(data.trend.buckets.map((b) => b.points)).toEqual([0, 0, 0]);
    const html = renderReportHtml(data);
    expect(html).toContain("</html>");
    expect(html).not.toContain("NaN");
  });
});

describe("notes", () => {
  it("shows coverage only below 100%", () => {
    const full = buildReport(
      input([ticket("a", { assignees: [user("1")], overrideLevel: "S" })]),
    );
    expect(full.notes).toEqual([]);
    expect(renderReportHtml(full)).not.toContain("Scoring coverage");

    const partial = buildReport(
      input([
        ticket("a", { assignees: [user("1")], overrideLevel: "S" }),
        ticket("b", { assignees: [user("1")] }),
        ticket("c", { assignees: [user("1")] }),
      ]),
    );
    expect(partial.notes).toHaveLength(1);
    expect(partial.notes[0].headline).toBe("Scoring coverage is 33%.");
    expect(partial.notes[0].text).toContain("1 of 3 delivered tickets");
    expect(renderReportHtml(partial)).toContain("Scoring coverage is 33%.");
  });

  it("never rounds a partial coverage up to 100%", () => {
    const tickets = Array.from({ length: 300 }, (_, i) =>
      ticket(`t${i}`, {
        assignees: [user("1")],
        overrideLevel: i === 0 ? null : "S",
      }),
    );
    const data = buildReport(input(tickets));
    expect(data.notes[0].headline).toBe("Scoring coverage is 99%.");
  });

  it("shows the approximate-dates note only when flagged", () => {
    const exact = buildReport(
      input([ticket("a", { assignees: [user("1")], overrideLevel: "S" })]),
    );
    expect(exact.notes.map((n) => n.headline)).not.toContain(
      "Some delivery dates are approximate.",
    );
    const approx = buildReport(
      input(
        [
          ticket("a", {
            assignees: [user("1")],
            overrideLevel: "S",
            status: "ready for testing",
            statusType: "custom",
            doneAt: null,
            statusSince: null,
            updatedAt: at(2026, 10, 8),
          }),
        ],
        { scope: { ...scope, deliveredStatuses: ["ready for testing"] } },
      ),
    );
    expect(approx.notes.map((n) => n.headline)).toEqual([
      "Some delivery dates are approximate.",
    ]);
    expect(renderReportHtml(approx)).toContain("approximate");
  });
});

describe("comparison tile", () => {
  const current = Array.from({ length: 6 }, (_, i) =>
    ticket(`c${i}`, { assignees: [user("1")] }),
  );
  const earlier = (n: number, day = 10) =>
    Array.from({ length: n }, (_, i) =>
      ticket(`p${i}`, { doneAt: at(2026, 9, day), assignees: [user("1")] }),
    );
  // October is over, so it compares whole with whole.
  const finished = { generatedAt: new Date(2026, 10, 5, 12) };

  it("compares a running period with the same days of the previous one", () => {
    // NOW is 18 Oct 12:00, so only Sep 1 to 18 12:00 counts.
    const data = buildReport(
      input(current, {
        previousTickets: [
          ...earlier(5, 10),
          ...earlier(4, 25).map((t) => ({ ...t, id: `late${t.id}` })),
        ],
      }),
    );
    expect(tile(data, "delivered").sub).toBe(
      "+20% vs the same days of Sep 2026",
    );
    const down = buildReport(input(current, { previousTickets: earlier(8) }));
    expect(tile(down, "delivered").sub).toBe(
      "-25% vs the same days of Sep 2026",
    );
    const same = buildReport(input(current, { previousTickets: earlier(6) }));
    expect(tile(same, "delivered").sub).toBe("0% vs the same days of Sep 2026");
  });

  it("compares a finished period whole", () => {
    const previousTickets = [
      ...earlier(5, 10),
      ...earlier(5, 25).map((t) => ({ ...t, id: `late${t.id}` })),
    ];
    const data = buildReport(input(current, { previousTickets, ...finished }));
    expect(tile(data, "delivered").sub).toBe("-40% vs Sep 2026");
  });

  it("compares the rolling 7 days whole", () => {
    const week = currentPeriod("7d", NOW);
    const before = previousPeriod(week) as Period;
    const prev = Array.from({ length: 3 }, (_, i) =>
      ticket(`p${i}`, {
        doneAt: new Date(before.end.getTime() - 3_600_000).toISOString(),
        assignees: [user("1")],
      }),
    );
    const data = buildReport(
      input(
        [ticket("a", { doneAt: at(2026, 10, 17), assignees: [user("1")] })],
        { period: week, previousPeriod: before, previousTickets: prev },
      ),
    );
    expect(tile(data, "delivered").sub).toBe("-67% vs the previous 7 days");
  });

  it("is left out for All time or when the previous period is empty", () => {
    const zero = buildReport(input(current, { previousTickets: [] }));
    expect(tile(zero, "delivered").sub).toBeNull();
    // Only after the elapsed span: nothing to compare with yet.
    const late = buildReport(
      input(current, { previousTickets: earlier(5, 28) }),
    );
    expect(tile(late, "delivered").sub).toBeNull();
    const all = buildReport(
      input(current, {
        period: currentPeriod("all", NOW, new Date(2026, 0, 1)),
        previousPeriod: null,
        previousTickets: earlier(5),
      }),
    );
    expect(tile(all, "delivered").sub).toBeNull();
    expect(renderReportHtml(zero)).not.toContain(" vs ");
  });
});

describe("trend buckets", () => {
  it("uses ISO weeks labelled by Monday for a month", () => {
    const data = buildReport(
      input([
        ticket("a", {
          doneAt: at(2026, 10, 1),
          assignees: [user("1")],
          overrideLevel: "XS",
        }),
        ticket("b", {
          doneAt: at(2026, 10, 7),
          assignees: [user("1")],
          overrideLevel: "S",
        }),
        ticket("c", {
          doneAt: at(2026, 10, 8),
          assignees: [user("1")],
          overrideLevel: "M",
        }),
        ticket("d", {
          doneAt: at(2026, 10, 14),
          assignees: [user("1")],
          overrideLevel: "XL",
        }),
      ]),
    );
    expect(data.trend.unit).toBe("week");
    // NOW is 18 Oct: the weeks of 19 and 26 Oct have not started.
    expect(data.trend.buckets.map((b) => b.label)).toEqual([
      "28 Sep",
      "5 Oct",
      "12 Oct",
    ]);
    expect(data.trend.buckets.map((b) => b.points)).toEqual([3, 24, 48]);
    expect(data.trend.buckets[1].title).toBe("Week of 5 Oct");
  });

  it("keeps every week of a finished month", () => {
    const data = buildReport(
      input(
        [
          ticket("d", {
            doneAt: at(2026, 10, 28),
            assignees: [user("1")],
            overrideLevel: "XL",
          }),
        ],
        { generatedAt: new Date(2026, 10, 5, 12) },
      ),
    );
    expect(data.trend.buckets.map((b) => b.label)).toEqual([
      "28 Sep",
      "5 Oct",
      "12 Oct",
      "19 Oct",
      "26 Oct",
    ]);
    expect(data.trend.buckets.map((b) => b.points)).toEqual([0, 0, 0, 0, 48]);
  });

  it("uses months for a year, split points included", () => {
    const year = currentPeriod("year", NOW);
    const data = buildReport(
      input(
        [
          ticket("a", {
            doneAt: at(2026, 1, 15),
            assignees: [user("1")],
            overrideLevel: "S",
          }),
          ticket("b", {
            doneAt: at(2026, 10, 5),
            assignees: [user("1"), user("2")],
            overrideLevel: "M",
          }),
        ],
        { period: year, previousPeriod: previousPeriod(year) },
      ),
    );
    expect(data.trend.unit).toBe("month");
    // Nov and Dec have not started on 18 Oct.
    expect(data.trend.buckets).toHaveLength(10);
    expect(data.trend.buckets[0]).toMatchObject({ label: "Jan", points: 8 });
    expect(data.trend.buckets[9]).toMatchObject({ label: "Oct", points: 16 });
  });

  it("uses months for a quarter and days for 7d", () => {
    const quarter = buildReport(
      input([], { period: currentPeriod("quarter", NOW) }),
    );
    expect(quarter.trend.unit).toBe("month");
    expect(quarter.trend.buckets.map((b) => b.label)).toEqual(["Oct"]);
    const finishedQuarter = buildReport(
      input([], {
        period: currentPeriod("quarter", NOW),
        generatedAt: new Date(2027, 0, 5),
      }),
    );
    expect(finishedQuarter.trend.buckets.map((b) => b.label)).toEqual([
      "Oct",
      "Nov",
      "Dec",
    ]);
    const week = buildReport(input([], { period: currentPeriod("7d", NOW) }));
    expect(week.trend.unit).toBe("day");
    expect(week.trend.buckets).toHaveLength(7);
  });

  it("uses months for All time within 24 months, else years", () => {
    const all = currentPeriod("all", NOW, new Date(2020, 0, 1));
    const short = buildReport(
      input(
        [
          ticket("a", {
            doneAt: at(2025, 11, 5),
            assignees: [user("1")],
            overrideLevel: "S",
          }),
          ticket("b", {
            doneAt: at(2026, 10, 5),
            assignees: [user("1")],
            overrideLevel: "S",
          }),
        ],
        { period: all, previousPeriod: null },
      ),
    );
    expect(short.trend.unit).toBe("month");
    expect(short.trend.buckets).toHaveLength(12);
    expect(short.trend.buckets[0].label).toBe("Nov 2025");

    const long = buildReport(
      input(
        [
          ticket("a", {
            doneAt: at(2023, 3, 5),
            assignees: [user("1")],
            overrideLevel: "S",
          }),
          ticket("b", {
            doneAt: at(2026, 10, 5),
            assignees: [user("1")],
            overrideLevel: "S",
          }),
        ],
        { period: all, previousPeriod: null },
      ),
    );
    expect(long.trend.unit).toBe("year");
    expect(long.trend.buckets.map((b) => b.label)).toEqual([
      "2023",
      "2024",
      "2025",
      "2026",
    ]);
    expect(long.trend.buckets[0].points).toBe(8);
    expect(long.trend.buckets[3].points).toBe(8);
  });
});

describe("hardest tickets", () => {
  const strongest: Criteria = {
    ambiguity: 0,
    investigation: 2,
    scope: 2,
    risk: 1,
    coordination: 0,
  };

  it("orders by points then newest delivery, keeps 10", () => {
    const tickets = [
      ticket("s", { overrideLevel: "S", assignees: [user("1")] }),
      ticket("xl-old", {
        overrideLevel: "XL",
        doneAt: at(2026, 10, 2),
        assignees: [user("1")],
      }),
      ticket("xl-new", {
        overrideLevel: "XL",
        doneAt: at(2026, 10, 9),
        assignees: [user("1")],
      }),
      ticket("l", { overrideLevel: "L", assignees: [user("1")] }),
      ticket("unscored", { assignees: [user("1")] }),
      ...Array.from({ length: 10 }, (_, i) =>
        ticket(`xs${i}`, { overrideLevel: "XS", assignees: [user("1")] }),
      ),
    ];
    const data = buildReport(input(tickets));
    expect(data.hardest).toHaveLength(10);
    expect(data.hardest.slice(0, 4).map((h) => h.id)).toEqual([
      "xl-new",
      "xl-old",
      "l",
      "s",
    ]);
    expect(data.hardest.map((h) => h.id)).not.toContain("unscored");
    expect(data.hardest[0]).toMatchObject({ level: "XL", points: 48 });
  });

  it("picks the highest criterion reason, earliest on ties", () => {
    const data = buildReport(
      input([
        ticket("a", {
          assignees: [user("1")],
          assessment: assessment(strongest, "why"),
        }),
      ]),
    );
    expect(data.hardest[0]).toMatchObject({
      reason: "why-investigation",
      edited: false,
      level: "M", // sum 5
    });
  });

  it("marks overrides as edited and uses the note or Set by hand", () => {
    const data = buildReport(
      input([
        ticket("noted", {
          assignees: [user("1")],
          assessment: assessment(strongest),
          overrideLevel: "XL",
          overrideNote: "  took a week ",
        }),
        ticket("silent", {
          doneAt: at(2026, 10, 3),
          assignees: [user("1")],
          overrideLevel: "XL",
        }),
        ticket("blank", {
          doneAt: at(2026, 10, 1),
          assignees: [user("1")],
          overrideLevel: "XL",
          overrideNote: "   ",
        }),
      ]),
    );
    const byId = Object.fromEntries(data.hardest.map((h) => [h.id, h]));
    expect(byId.noted).toMatchObject({
      edited: true,
      level: "XL",
      reason: "took a week",
    });
    expect(byId.silent).toMatchObject({ edited: true, reason: "Set by hand" });
    expect(byId.blank.reason).toBe("Set by hand");
    const html = renderReportHtml(data);
    expect(html).toContain('<span class="edited">edited</span>');
  });

  it("lists all distinct assignees or Unassigned", () => {
    const data = buildReport(
      input([
        ticket("a", {
          overrideLevel: "XL",
          assignees: [user("1", "Ann"), user("2", "Bo"), user("1", "Ann")],
        }),
        ticket("b", { overrideLevel: "L" }),
      ]),
    );
    expect(data.hardest.map((h) => h.members)).toEqual([
      "Ann, Bo",
      "Unassigned",
    ]);
  });
});

describe("faded members", () => {
  it("fades members below 50% coverage in the data, table and chart", () => {
    const tickets = [
      ticket("a", { assignees: [user("1", "Solid")], overrideLevel: "M" }),
      ticket("b", { assignees: [user("2", "Thin")], overrideLevel: "M" }),
      ticket("c", { assignees: [user("2", "Thin")] }),
      ticket("d", { assignees: [user("2", "Thin")] }),
      ticket("e", { assignees: [user("3", "Half")], overrideLevel: "M" }),
      ticket("f", { assignees: [user("3", "Half")] }),
    ];
    const data = buildReport(input(tickets));
    const faded = Object.fromEntries(
      data.members.map((m) => [m.name, m.faded]),
    );
    expect(faded).toEqual({ Solid: false, Thin: true, Half: false });
    const html = renderReportHtml(data);
    expect(html.match(/<tr class="faded">/g)).toHaveLength(1);
    expect(html).toContain('opacity="0.35"');
    expect(html).toContain("(33% scored)");
  });
});

describe("members with no delivered tickets", () => {
  it("are not faded: coverage means nothing for them", () => {
    const tickets = [
      ticket("a", { assignees: [user("1", "Solid")], overrideLevel: "M" }),
      ticket("w", {
        status: "in progress",
        statusType: "custom",
        doneAt: null,
        assignees: [user("2", "Busy")],
      }),
    ];
    const data = buildReport(input(tickets));
    const busy = data.members.find((m) => m.name === "Busy")!;
    expect(busy.delivered).toBe(0);
    expect(busy.active).toBe(1);
    expect(busy.faded).toBe(false);
    const html = renderReportHtml(data);
    expect(html).not.toContain('<tr class="faded">');
    expect(html).not.toContain('opacity="0.35"');
    expect(html).toContain("no delivered work");
  });
});

describe("member names in the charts", () => {
  const long = "Nguyen Van Hoang";

  it("clips a 16 character name to fit the left margin and keeps the full name in a title", () => {
    const data = buildReport(
      input([
        ticket("a", { assignees: [user("1", long)], overrideLevel: "M" }),
      ]),
    );
    const html = renderReportHtml(data);
    expect(html).toContain(`>Nguyen Van ...<title>${long}</title></text>`);
    expect(html).not.toContain(`>${long}</text>`);
  });

  it("leaves a name of 14 characters whole", () => {
    const name = "Nguyen Van Hoa";
    expect(name).toHaveLength(14);
    const html = renderReportHtml(
      buildReport(
        input([
          ticket("a", { assignees: [user("1", name)], overrideLevel: "M" }),
        ]),
      ),
    );
    expect(html).toContain(`text-anchor="end">${name}</text>`);
  });
});

describe("comparison note", () => {
  const prev = (id: string) =>
    ticket(id, { doneAt: at(2026, 9, 5), updatedAt: at(2026, 9, 5) });
  const current = [ticket("a", { assignees: [user("1", "Ann")] })];

  it("is absent when the previous period was fully fetched, and the comparison shows", () => {
    const data = buildReport(
      input(current, { previousTickets: [prev("p1"), prev("p2")] }),
    );
    expect(data.comparisonNote).toBeNull();
    expect(tile(data, "delivered").sub).toContain("vs ");
    expect(renderReportHtml(data)).not.toContain("No comparison with");
  });

  it("explains the missing comparison when the previous period was not fully fetched", () => {
    const data = buildReport(input(current, { previousIncomplete: true }));
    expect(data.comparisonNote).toBe(
      "No comparison with Sep 2026: its tickets were not fully fetched. Open that period in Ticket Contributors once to include it.",
    );
    expect(tile(data, "delivered").sub).toBeNull();
    const html = renderReportHtml(data);
    expect(html).toContain("No comparison with Sep 2026: its tickets");
    // At the top, before the tiles.
    expect(html.indexOf("No comparison with")).toBeLessThan(
      html.indexOf('class="tiles"'),
    );
  });

  it("has no note for All time, which has no previous period", () => {
    const data = buildReport(
      input(current, {
        period: currentPeriod("all", NOW, null),
        previousPeriod: null,
        previousIncomplete: true,
      }),
    );
    expect(data.comparisonNote).toBeNull();
  });

  it("escapes the previous period label", () => {
    const data = buildReport(
      input(current, {
        previousPeriod: { ...SEPTEMBER, label: "<b>Sep</b> & co" },
        previousIncomplete: true,
      }),
    );
    const html = renderReportHtml(data);
    expect(html).toContain("&lt;b&gt;Sep&lt;/b&gt; &amp; co");
    expect(html).not.toContain("<b>Sep</b>");
  });
});

describe("renderReportHtml layout", () => {
  it("renders header, sections, SVG titles and the method table", () => {
    const data = buildReport(
      input([
        ticket("a", { assignees: [user("1", "Ann Lee")], overrideLevel: "L" }),
      ]),
    );
    const html = renderReportHtml(data);
    expect(html).toContain(
      "<title>Ticket Contributors report - Casium - Oct 2026</title>",
    );
    expect(html).toContain("Scored with claude-sonnet-5-5 (Claude Code)");
    for (const heading of [
      "Points by member",
      "Difficulty mix by member",
      "Team points per week",
      "Delivered and active work",
      "Member table",
      "Hardest tickets this period",
      "How the score works",
    ]) {
      expect(html).toContain(`<h2>${heading}</h2>`);
    }
    expect(html.match(/<svg /g)).toHaveLength(4);
    expect(html).toContain("<title>Ann Lee: 32 points, coverage 100%</title>");
    for (const color of [
      "#2a78d6",
      "#eb6834",
      "#86b6ef",
      "#5598e7",
      "#1c5cab",
      "#104281",
    ]) {
      expect(html).toContain(color);
    }
    expect(html).toContain(
      "<td>Points</td><td>3</td><td>8</td><td>16</td><td>32</td><td>48</td>",
    );
    expect(html).toContain("Delivered statuses: closed.");
  });

  it("omits the scored-with part without AI config", () => {
    const html = renderReportHtml(buildReport(input([], { ai: null })));
    expect(html).not.toContain("Scored with");
  });
});

describe("unassigned tickets", () => {
  const tickets = [
    ticket("a", { assignees: [user("1", "Ann")], overrideLevel: "S" }),
    ticket("u1", { overrideLevel: "M" }),
    ticket("u2"),
  ];

  it("gets its own table row and still counts in the tiles", () => {
    const data = buildReport(input(tickets));
    expect(data.unassigned).toEqual({ delivered: 2, scored: 1 });
    expect(tile(data, "delivered").value).toBe("3");
    expect(tile(data, "scored").sub).toBe("2 of 3");
    const html = renderReportHtml(data);
    expect(html).toContain(
      '<tr class="unassigned"><td>Unassigned</td><td>-</td><td>-</td><td>2</td><td>1/2</td>',
    );
    expect(html).toContain(
      "A ticket with several assignees counts for each of them; unassigned tickets have their own row.",
    );
  });

  it("has no row without unassigned tickets", () => {
    const data = buildReport(
      input([ticket("a", { assignees: [user("1")], overrideLevel: "S" })]),
    );
    expect(data.unassigned).toBeNull();
    expect(renderReportHtml(data)).not.toContain('class="unassigned"');
  });

  it("renders the table when only unassigned tickets exist", () => {
    const html = renderReportHtml(buildReport(input([ticket("u")])));
    expect(html).toContain("<td>Unassigned</td>");
    expect(html).not.toContain("No members to show");
  });
});

describe("coverage text", () => {
  it("never rounds a faded member up to 50% or a partial one up to 100%", () => {
    // 124 of 250 scored is 49.6%; 299 of 300 is 99.67%.
    const tickets = [
      ...Array.from({ length: 250 }, (_, i) =>
        ticket(`f${i}`, {
          assignees: [user("1", "Faded")],
          overrideLevel: i < 124 ? "S" : null,
        }),
      ),
      ...Array.from({ length: 300 }, (_, i) =>
        ticket(`n${i}`, {
          assignees: [user("2", "Nearly")],
          overrideLevel: i < 299 ? "S" : null,
        }),
      ),
    ];
    const data = buildReport(input(tickets));
    const byName = Object.fromEntries(data.members.map((m) => [m.name, m]));
    expect(byName.Faded.faded).toBe(true);
    expect(byName.Faded.coveragePercent).toBe(49);
    expect(byName.Nearly.coveragePercent).toBe(99);
    const html = renderReportHtml(data);
    expect(html).toContain("Faded: 992 points, coverage 49%");
    expect(html).toContain("Nearly: 2,392 points, coverage 99%");
    expect(html).not.toContain("coverage 50%");
    expect(html).not.toContain("coverage 100%");
  });
});

describe("month by month", () => {
  const Q4 = currentPeriod("quarter", NOW);
  const Q3 = previousPeriod(Q4) as Period;
  const YEAR = currentPeriod("year", NOW);
  const evil = `<b>"Evil"</b> & 'Co'`;

  // Q3 2026: July, August, September.
  const q3Tickets = [
    ticket("j1", {
      doneAt: at(2026, 7, 5),
      assignees: [user("1", "Ann"), user("2", "Bob")],
      overrideLevel: "M", // 16 split 8 + 8
    }),
    ticket("j2", {
      doneAt: at(2026, 7, 20),
      assignees: [user("1", "Ann")],
      overrideLevel: "XL", // 48
    }),
    ticket("a1", { doneAt: at(2026, 8, 3), assignees: [user("2", "Bob")] }), // unscored
    ticket("a2", {
      doneAt: at(2026, 8, 9),
      assignees: [user("2", "Bob")],
      overrideLevel: "L",
    }),
    ticket("s1", {
      doneAt: at(2026, 9, 30),
      assignees: [user("1", "Ann")],
      overrideLevel: "XS",
    }),
    ticket("u1", { doneAt: at(2026, 9, 2), overrideLevel: "S" }), // unassigned
    ticket("wip", {
      status: "in progress",
      statusType: "custom",
      doneAt: null,
      updatedAt: at(2026, 8, 15),
      assignees: [user("3", "Cy")],
    }),
  ];

  const quarter = (patch: Partial<ReportInput> = {}) =>
    buildReport(
      input(q3Tickets, { period: Q3, previousPeriod: null, ...patch }),
    );

  it("adds nothing for a month, 7 days or all time", () => {
    for (const period of [
      OCTOBER,
      currentPeriod("7d", NOW),
      currentPeriod("all", NOW),
    ]) {
      const data = buildReport(input(q3Tickets, { period }));
      expect(data.monthly).toBeNull();
      const out = renderReportHtml(data);
      expect(out).not.toContain("Month by month");
      expect(out).not.toContain("<details");
    }
  });

  it("has one column per month of a finished quarter, after the trend", () => {
    const data = quarter();
    expect(data.monthly!.months.map((m) => m.label)).toEqual([
      "Jul",
      "Aug",
      "Sep",
    ]);
    const out = renderReportHtml(data);
    const section = out.slice(out.indexOf("Month by month"));
    expect(out.indexOf("Team points per month")).toBeLessThan(
      out.indexOf("Month by month"),
    );
    expect(out.indexOf("Month by month")).toBeLessThan(
      out.indexOf("<h2>Member table</h2>"),
    );
    expect(section).toContain(
      "<tr><th>Member</th><th>Jul</th><th>Aug</th><th>Sep</th><th>Total</th></tr>",
    );
    expect(out).toContain(
      "Click a month to see its details. Expand the months you want before printing.",
    );
  });

  it("stops at the month of the report date in an unfinished quarter and year", () => {
    const q = buildReport(input(q3Tickets, { period: Q4 }));
    expect(q.monthly!.months.map((m) => m.label)).toEqual(["Oct"]);
    const y = buildReport(input(q3Tickets, { period: YEAR }));
    expect(y.monthly!.months.map((m) => m.title)).toEqual(
      [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
      ].map((n) => `${n} 2026`),
    );
    // The year reaches back, so July's work shows in its July cell.
    const ann = y.monthly!.rows.find((r) => r.name === "Ann")!;
    expect(ann.cells[6]).toMatchObject({ points: 56, delivered: 2, scored: 2 });
  });

  it("matrix cells and totals equal the overview member table", () => {
    const data = quarter();
    const monthly = data.monthly!;
    expect(monthly.rows.map((r) => r.name)).toEqual([
      ...data.members.map((m) => m.name),
      "Unassigned",
    ]);
    for (const member of data.members) {
      const row = monthly.rows.find((r) => r.id === member.id)!;
      expect(row.total).toMatchObject({
        points: member.points,
        delivered: member.delivered,
        scored: member.scored,
      });
      expect(row.cells.reduce((s, c) => s + c.points, 0)).toBe(member.points);
      expect(row.cells.reduce((s, c) => s + c.delivered, 0)).toBe(
        member.delivered,
      );
    }
    const unassigned = monthly.rows[monthly.rows.length - 1];
    expect(unassigned.total).toMatchObject(data.unassigned!);
    const ann = monthly.rows.find((r) => r.name === "Ann")!;
    expect(ann.cells.map((c) => [c.points, c.delivered])).toEqual([
      [56, 2],
      [0, 0],
      [3, 1],
    ]);
    expect(tile(data, "points").value).toBe(
      String(monthly.months.reduce((s, m) => s + m.points, 0)),
    );
    const out = renderReportHtml(data);
    expect(out).toContain("Ann, July 2026: 56 points, 2 delivered, 2 scored");
    expect(out).toContain('<span class="sub">&middot; 2</span>');
  });

  it("shades by points against the busiest cell and leaves zero cells plain", () => {
    const monthly = quarter().monthly!;
    const ann = monthly.rows.find((r) => r.name === "Ann")!;
    expect(ann.cells[0].shade).toBe(4);
    expect(ann.cells[1].shade).toBeNull();
    expect(ann.cells[2].shade).toBe(0);
    expect(ann.total.shade).toBeNull();
    const out = renderReportHtml(quarter());
    expect(out).toContain("background:#6da7ec");
    expect(out).toContain("background:#cde2fb");
  });

  it("draws the count on a shaded cell in the darker ink", () => {
    const out = renderReportHtml(quarter());
    expect(out).toMatch(/td\.shaded \.sub \{ color: var\(--ink-on-shade\)/);
    expect(out).toContain("--ink-on-shade: #2b2a28;");
    expect(out).toContain(
      '<td class="cell shaded" style="background:#6da7ec" title="Ann, July 2026',
    );
    // Blank and total cells keep the lighter ink: they carry no shaded class.
    expect(out).toContain('<td class="total" title="Ann, Q3 2026');
    expect(out).not.toContain('class="cell" style=');
  });

  it("fades the matrix row of a member below 50% overall coverage", () => {
    const monthly = quarter().monthly!;
    expect(monthly.rows.map((r) => [r.name, r.faded])).toEqual([
      ["Ann", false],
      ["Bob", false],
      ["Cy", false],
      ["Unassigned", false],
    ]);
    const data = buildReport(
      input(
        [
          ticket("f1", {
            doneAt: at(2026, 8, 3),
            assignees: [user("1", "Low")],
          }),
          ticket("f2", {
            doneAt: at(2026, 8, 4),
            assignees: [user("1", "Low")],
          }),
          ticket("f3", {
            doneAt: at(2026, 8, 5),
            assignees: [user("1", "Low")],
            overrideLevel: "S",
          }),
        ],
        { period: Q3, previousPeriod: null },
      ),
    );
    expect(data.members[0].faded).toBe(true);
    expect(data.monthly!.rows[0].faded).toBe(true);
    expect(renderReportHtml(data)).toContain('<tr class="faded"><td>Low</td>');
  });

  it("folds a ticket delivered after the last month into that month", () => {
    const late = ticket("late", {
      doneAt: at(2026, 11, 5), // inside Q4, after the report date
      assignees: [user("1", "Ann")],
      overrideLevel: "L",
    });
    const data = buildReport(input([late], { period: Q4 }));
    expect(data.monthly!.months.map((m) => m.label)).toEqual(["Oct"]);
    expect(data.monthly!.months[0]).toMatchObject({
      delivered: 1,
      points: 32,
    });
    expect(data.monthly!.rows[0].cells[0]).toMatchObject({
      points: 32,
      delivered: 1,
    });
    expect(data.monthly!.rows[0].total.points).toBe(32);
    expect(data.monthly!.months[0].hardest.map((h) => h.id)).toEqual(["late"]);
  });

  it("hides the help line when there are no month blocks, but keeps empty months", () => {
    const empty = renderReportHtml(
      buildReport(input([], { period: Q3, previousPeriod: null })),
    );
    expect(empty).toContain("Month by month");
    expect(empty).not.toContain("Click a month");
    const year = buildReport(input(q3Tickets, { period: YEAR }));
    expect(year.monthly!.months).toHaveLength(10);
    expect(year.monthly!.months[0].delivered).toBe(0);
    expect(renderReportHtml(year)).toContain("Click a month");
  });

  it("closes every month block by default", () => {
    const out = renderReportHtml(quarter());
    expect(out.match(/<details class="month">/g)).toHaveLength(3);
    expect(out).not.toMatch(/<details[^>]*\bopen\b/);
    expect(out).toContain("<summary>July 2026 ");
    expect(out).toMatch(/July 2026 <span class="meta">&middot; 2 delivered/);
    expect(out).not.toContain("<script");
  });

  it("lists the 3 hardest tickets of each month", () => {
    const tickets = [
      ...["a", "b", "c", "d"].map((id, i) =>
        ticket(id, {
          doneAt: at(2026, 8, 3 + i),
          assignees: [user("1", "Ann")],
          overrideLevel: (["XS", "S", "M", "L"] as const)[i],
        }),
      ),
      ticket("july", {
        doneAt: at(2026, 7, 3),
        assignees: [user("1", "Ann")],
        overrideLevel: "XS",
      }),
    ];
    const monthly = buildReport(
      input(tickets, { period: Q3, previousPeriod: null }),
    ).monthly!;
    expect(monthly.months[1].hardest.map((h) => h.id)).toEqual(["d", "c", "b"]);
    expect(monthly.months[0].hardest.map((h) => h.id)).toEqual(["july"]);
    expect(monthly.months[2].hardest).toEqual([]);
  });

  it("builds the tiles and member table of each month on their own", () => {
    const aug = quarter().monthly!.months[1];
    expect(aug.delivered).toBe(2);
    expect(aug.tiles.map((t) => [t.id, t.value])).toEqual([
      ["delivered", "2"],
      ["scored", "50%"],
      ["points", "32"],
      ["active", "1"],
    ]);
    expect(aug.members.find((m) => m.name === "Bob")).toMatchObject({
      points: 32,
      delivered: 2,
      scored: 1,
    });
    expect(aug.members.find((m) => m.name === "Cy")).toMatchObject({
      delivered: 0,
      active: 1,
    });
  });

  it("escapes names and titles inside the month section", () => {
    const data = buildReport(
      input(
        [
          ticket("x", {
            title: evil,
            doneAt: at(2026, 8, 3),
            assignees: [user("1", evil)],
            overrideLevel: "XL",
            overrideNote: evil,
          }),
        ],
        { period: Q3, previousPeriod: null },
      ),
    );
    const out = renderReportHtml(data);
    const section = out.slice(
      out.indexOf("Month by month"),
      out.indexOf("<h2>Member table</h2>"),
    );
    expect(section).toContain(
      "&lt;b&gt;&quot;Evil&quot;&lt;/b&gt; &amp; &#39;Co&#39;",
    );
    expect(section).not.toContain('<b>"Evil"');
    expect(section).not.toContain("<b>&quot;");
    expect(section).toContain(
      'title="&lt;b&gt;&quot;Evil&quot;&lt;/b&gt; &amp; &#39;Co&#39;, August 2026',
    );
  });

  it("shows a month's coverage note only when that month is below 100%", () => {
    const data = quarter();
    const [jul, aug, sep] = data.monthly!.months;
    expect(jul.note).toBeNull();
    expect(aug.note?.headline).toBe("Scoring coverage is 50%.");
    expect(aug.note?.text).toContain("1 of 2 delivered tickets in August 2026");
    expect(sep.note).toBeNull();
    const out = renderReportHtml(data);
    const blocks = out.split('<details class="month">').slice(1);
    expect(blocks[0]).not.toContain("Scoring coverage");
    expect(blocks[1]).toContain("Scoring coverage is 50%.");
    expect(blocks[2]).not.toContain("Scoring coverage");
  });

  it("works with nothing delivered", () => {
    const data = buildReport(input([], { period: Q3, previousPeriod: null }));
    const out = renderReportHtml(data);
    expect(out).toContain("Month by month");
    expect(out).not.toContain("<details");
    expect(out).not.toContain("NaN");
  });
});

describe("excluded members", () => {
  const evil = `<b>"Ex"</b> & 'Co'`;
  const excluded = {
    ...scope,
    excludedMembers: [
      { id: "2", name: "Old Two" },
      { id: "9", name: "Left Lee" },
    ],
  };
  const tickets = [
    ticket("own", {
      assignees: [user("1", "Ann")],
      overrideLevel: "S", // 8
    }),
    ticket("shared", {
      assignees: [user("1", "Ann"), user("2", evil)],
      overrideLevel: "M", // 16 goes to Ann alone
    }),
    ticket("theirs", {
      title: "Only theirs",
      assignees: [user("2", evil)],
      overrideLevel: "XL",
    }),
    ticket("nobody", { overrideLevel: "XS" }),
    ticket("busy", {
      status: "in progress",
      statusType: "custom",
      doneAt: null,
      updatedAt: at(2026, 10, 8),
      assignees: [user("2", evil)],
    }),
  ];

  it("leaves excluded members out of every number and table", () => {
    const data = buildReport(input(tickets, { scope: excluded }));
    expect(data.members.map((m) => [m.name, m.delivered, m.points])).toEqual([
      ["Ann", 2, 24],
    ]);
    expect(tile(data, "delivered").value).toBe("3");
    expect(tile(data, "points").value).toBe("24");
    expect(tile(data, "active").value).toBe("0");
    expect(data.unassigned).toEqual({ delivered: 1, scored: 1 });
    expect(data.load.map((l) => l.name)).toEqual(["Ann"]);
    expect(data.hardest.map((h) => h.id)).not.toContain("theirs");
    expect(data.hardest.find((h) => h.id === "shared")?.members).toBe("Ann");
    const out = renderReportHtml(data);
    expect(out).not.toContain("Only theirs");
    // Only the method section names them.
    expect(out.split("Ex&quot;")).toHaveLength(2);
  });

  it("keeps them out of the month by month matrix", () => {
    const q4 = currentPeriod("quarter", NOW);
    const data = buildReport(
      input(tickets, { scope: excluded, period: q4, previousPeriod: null }),
    );
    expect(data.monthly!.rows.map((r) => r.name)).toEqual([
      "Ann",
      "Unassigned",
    ]);
    expect(renderReportHtml(data).split("Ex&quot;")).toHaveLength(2);
  });

  it("lists them by name (a loaded name first, else the stored one), escaped, in the method section", () => {
    const out = renderReportHtml(
      buildReport(input(tickets, { scope: excluded })),
    );
    expect(out).toContain(
      "Excluded from this board: &lt;b&gt;&quot;Ex&quot;&lt;/b&gt; &amp; &#39;Co&#39;, Left Lee.",
    );
    expect(out).not.toContain('<b>"Ex"');
  });

  it("adds no line when nobody is excluded", () => {
    const out = renderReportHtml(buildReport(input(tickets)));
    expect(out).not.toContain("Excluded from this board");
  });
});
