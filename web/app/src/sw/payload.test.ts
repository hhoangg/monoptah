import { PushPayloadSchema } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { GENERIC_BODY, GENERIC_TITLE, notificationFor, parsePushPayload, readPushData } from "./payload";

const valid = { v: 1, t: "Studio Mac", b: "api: Finished", url: "/d/dk1/s/s9", tag: "s9" };

// The hand-written validator must give the same verdict as the protocol schema.
const FIXTURES: unknown[] = [
  valid,
  { ...valid, extra: "ignored" },
  { v: 1, t: "", b: "", url: "", tag: "" },
  { ...valid, v: 2 },
  { ...valid, v: "1" },
  { ...valid, t: 1 },
  { ...valid, b: null },
  { ...valid, url: undefined },
  { ...valid, tag: [] },
  { t: "x", b: "y", url: "/d/a/s/b", tag: "b" },
  {},
  [],
  null,
  "text",
  42,
];

describe("parsePushPayload", () => {
  it.each(FIXTURES.map((fixture, index) => [index, fixture] as const))("agrees with PushPayloadSchema on fixture %i", (_index, fixture) => {
    const schema = PushPayloadSchema.safeParse(fixture);
    const parsed = parsePushPayload(fixture);
    expect(parsed !== null).toBe(schema.success);
    if (schema.success) expect(parsed).toEqual(schema.data);
  });
});

describe("notificationFor", () => {
  it("uses the payload text verbatim, the tag and the URL", () => {
    expect(notificationFor(parsePushPayload(valid))).toEqual({ title: "Studio Mac", options: { body: "api: Finished", tag: "s9", data: { url: "/d/dk1/s/s9" } } });
  });

  it("shows a generic notice for an invalid payload", () => {
    expect(notificationFor(null)).toEqual({ title: GENERIC_TITLE, options: { body: GENERIC_BODY, data: { url: "/" } } });
    expect(GENERIC_TITLE).toBe("Monoptah");
  });

  it("keeps the notice but drops a hostile URL", () => {
    const spec = notificationFor(parsePushPayload({ ...valid, url: "https://evil.example" }));
    expect(spec.title).toBe("Studio Mac");
    expect(spec.options.data.url).toBe("/");
  });
});

describe("readPushData", () => {
  it("returns null for no data and for unreadable data", () => {
    expect(readPushData(null)).toBeNull();
    expect(
      readPushData({
        json: () => {
          throw new SyntaxError("bad");
        },
      }),
    ).toBeNull();
    expect(readPushData({ json: () => valid })).toEqual(valid);
  });
});
