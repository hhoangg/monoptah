// The avatar of a desktop (migration 0008): an emoji or a small PNG, JPEG or WebP, per account.
import { DESKTOP_AVATAR_MAX_BYTES } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { authedCall } from "../support/accounts.ts";
import { env, json, newAccount, pairDesktop } from "./support/api-support.ts";

const toBase64 = (bytes: number[]) => btoa(String.fromCharCode(...bytes));
const pad = (head: number[], total: number) => [...head, ...new Array<number>(total - head.length).fill(7)];
const PNG = pad([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], 64);
const JPEG = pad([0xff, 0xd8, 0xff, 0xe0], 64);
const WEBP = pad([0x52, 0x49, 0x46, 0x46, 1, 2, 3, 4, 0x57, 0x45, 0x42, 0x50], 64);
const SVG = Array.from(new TextEncoder().encode('<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>'));
const ROCKET = "\u{1F680}";

interface Summary {
  avatar?: { kind: string; emoji?: string; hash?: string } | null;
}

async function patch(account: Awaited<ReturnType<typeof newAccount>>, id: string, body: unknown) {
  return authedCall(account.session, `/api/desktops/${id}`, json(body, "PATCH"));
}

describe("PATCH /api/desktops/:id avatar", () => {
  it("starts without an avatar", async () => {
    const mine = await newAccount();
    await pairDesktop(mine);
    const list = (await (await authedCall(mine.session, "/api/desktops")).json()) as { desktops: Summary[] };
    expect(list.desktops[0]?.avatar).toBeNull();
  });

  it("stores an emoji, lists it, and clears it with null", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const res = await patch(mine, paired.desktopId, { avatar: { kind: "emoji", emoji: ROCKET } });
    expect(res.status).toBe(200);
    expect(((await res.json()) as { desktop: Summary }).desktop.avatar).toEqual({ kind: "emoji", emoji: ROCKET });
    const list = (await (await authedCall(mine.session, "/api/desktops")).json()) as { desktops: Summary[] };
    expect(list.desktops[0]?.avatar).toEqual({ kind: "emoji", emoji: ROCKET });
    // The directory entry carries it too, and a rename keeps it.
    await patch(mine, paired.desktopId, { name: "Renamed" });
    const detail = (await (await authedCall(mine.session, `/api/desktops/${paired.desktopId}`)).json()) as Summary;
    expect(detail.avatar).toEqual({ kind: "emoji", emoji: ROCKET });
    const cleared = await patch(mine, paired.desktopId, { avatar: null });
    expect(((await cleared.json()) as { desktop: Summary }).desktop.avatar).toBeNull();
    expect(await env.DB.prepare("SELECT avatar_kind, avatar_emoji, avatar_hash, avatar_image FROM desktops WHERE id = ?").bind(paired.desktopId).first()).toEqual({
      avatar_kind: null,
      avatar_emoji: null,
      avatar_hash: null,
      avatar_image: null,
    });
  });

  it.each([["text", "hello"], ["a digit", "1"], ["two emoji", `${ROCKET}${ROCKET}x`], ["markup", "<img>"], ["empty", ""], ["too long", "\u{1F680}‍".repeat(9)]])("refuses %s as an emoji", async (_name, emoji) => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    expect((await patch(mine, paired.desktopId, { avatar: { kind: "emoji", emoji } })).status).toBe(400);
  });

  it.each([["PNG", PNG, "image/png"], ["JPEG", JPEG, "image/jpeg"], ["WebP", WEBP, "image/webp"]])("stores a %s and serves it with safe headers", async (_name, bytes, type) => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const res = await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: type, data: toBase64(bytes) } });
    expect(res.status).toBe(200);
    const avatar = ((await res.json()) as { desktop: Summary }).desktop.avatar;
    expect(avatar).toMatchObject({ kind: "image" });
    expect(avatar?.hash).toMatch(/^[0-9a-f]{16}$/);

    const image = await authedCall(mine.session, `/api/desktops/${paired.desktopId}/avatar?v=${avatar?.hash}`);
    expect(image.status).toBe(200);
    expect(image.headers.get("Content-Type")).toBe(type);
    expect(image.headers.get("X-Content-Type-Options")).toBe("nosniff");
    expect(image.headers.get("Content-Security-Policy")).toContain("sandbox");
    expect(image.headers.get("Cache-Control")).toBe("no-store");
    expect(Array.from(new Uint8Array(await image.arrayBuffer()))).toEqual(bytes);
  });

  it("takes the type from the bytes, not from the mime the client claims", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: toBase64(JPEG) } });
    const image = await authedCall(mine.session, `/api/desktops/${paired.desktopId}/avatar`);
    expect(image.headers.get("Content-Type")).toBe("image/jpeg");
  });

  it("refuses SVG, HTML and a GIF even when they claim to be a PNG", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const html = Array.from(new TextEncoder().encode("<!doctype html><script>alert(1)</script>"));
    const gif = Array.from(new TextEncoder().encode("GIF89a\u0001\u0000\u0001\u0000"));
    for (const bytes of [SVG, html, gif]) {
      const res = await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: toBase64(bytes) } });
      expect(res.status).toBe(400);
    }
    expect((await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/svg+xml", data: toBase64(PNG) } })).status).toBe(400);
    expect((await authedCall(mine.session, `/api/desktops/${paired.desktopId}/avatar`)).status).toBe(404);
  });

  it("refuses bytes over the cap, empty or invalid base64", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const big = pad([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], DESKTOP_AVATAR_MAX_BYTES + 1);
    expect((await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: toBase64(big) } })).status).toBe(400);
    expect((await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: "" } })).status).toBe(400);
    expect((await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: "not base64!" } })).status).toBe(400);
    const exact = pad([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], DESKTOP_AVATAR_MAX_BYTES);
    expect((await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: toBase64(exact) } })).status).toBe(200);
  });

  it("replaces an image with an emoji and drops the old bytes", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: toBase64(PNG) } });
    await patch(mine, paired.desktopId, { avatar: { kind: "emoji", emoji: ROCKET } });
    expect(await env.DB.prepare("SELECT avatar_kind, avatar_hash, avatar_image FROM desktops WHERE id = ?").bind(paired.desktopId).first()).toEqual({ avatar_kind: "emoji", avatar_hash: null, avatar_image: null });
    expect((await authedCall(mine.session, `/api/desktops/${paired.desktopId}/avatar`)).status).toBe(404);
  });

  it("is the owner's only: another account cannot set or read an avatar, and no session gets 401", async () => {
    const mine = await newAccount();
    const theirs = await newAccount();
    const paired = await pairDesktop(mine);
    await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: toBase64(PNG) } });
    expect((await patch(theirs, paired.desktopId, { avatar: { kind: "emoji", emoji: ROCKET } })).status).toBe(404);
    expect((await authedCall(theirs.session, `/api/desktops/${paired.desktopId}/avatar`)).status).toBe(404);
    expect((await authedCall({ cookie: "" }, `/api/desktops/${paired.desktopId}/avatar`)).status).toBe(401);
    expect((await authedCall(mine.session, `/api/desktops/${paired.desktopId}/avatar`)).status).toBe(200);
  });

  it("does not put the image bytes in the list or the directory answers", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    await patch(mine, paired.desktopId, { avatar: { kind: "image", mime: "image/png", data: toBase64(PNG) } });
    const list = await (await authedCall(mine.session, "/api/desktops")).text();
    const detail = await (await authedCall(mine.session, `/api/desktops/${paired.desktopId}`)).text();
    for (const text of [list, detail]) {
      expect(text).not.toContain(toBase64(PNG));
      expect(text).not.toContain("avatar_image");
    }
  });
});
