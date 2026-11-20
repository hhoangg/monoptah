import { createDecipheriv, createHash, createHmac } from "node:crypto";
import { describe, expect, it } from "vitest";
import { FixtureError, type MemoryBackend, MockTransfers, MockWorld, FIXTURE_HANDLERS, sealedSize, type HandlerContext, type MockDevice } from "../src/index.ts";
import { LARGE_BINARY_BYTES, LARGE_BINARY_PATH, LARGE_TEXT_PATH, OUTSIDE_TEXT_PATH, SECRET_PATH } from "../src/mentioned.ts";

const device = (permission: "read" | "control"): MockDevice => ({ fingerprint: "ff", status: "confirmed", permission, selection: { projects: null, monos: null } });

function open(sealed: Uint8Array, key: Buffer, noncePrefix: Buffer, chunk = 1024 * 1024): Buffer {
  const step = chunk + 16;
  const count = Math.max(1, Math.ceil(sealed.length / step));
  const parts: Buffer[] = [];
  for (let index = 0; index < count; index++) {
    const piece = Buffer.from(sealed.subarray(index * step, Math.min(sealed.length, (index + 1) * step)));
    const nonce = Buffer.alloc(12);
    noncePrefix.copy(nonce, 0);
    nonce.writeUInt32BE(index, 8);
    const aad = Buffer.alloc(8);
    aad.write("mt1", 0, "latin1");
    aad.writeUInt32BE(index, 3);
    aad[7] = index === count - 1 ? 1 : 0;
    const decipher = createDecipheriv("aes-256-gcm", key, nonce);
    decipher.setAAD(aad);
    decipher.setAuthTag(piece.subarray(piece.length - 16));
    parts.push(decipher.update(piece.subarray(0, piece.length - 16)), decipher.final());
  }
  return Buffer.concat(parts);
}

async function prepare(transfers: MockTransfers, permission: "read" | "control", path: string, asset?: string) {
  const ctx: HandlerContext = { world: new MockWorld(), device: device(permission), transfers };
  return FIXTURE_HANDLERS["file.prepare"]({ path, ...(asset ? { asset } : {}) }, ctx);
}

const codeOf = async (run: () => Promise<unknown>) => {
  try {
    await run();
  } catch (error) {
    return error instanceof FixtureError ? error.code : "other";
  }
  return "none";
};

describe("mock file.prepare", () => {
  it("stores ciphertext under hash.keyId that opens to the content, and reuses it for a second call", async () => {
    const transfers = new MockTransfers();
    const backend = transfers.backend as MemoryBackend;
    const first = await prepare(transfers, "read", "report.html");
    expect(first).toMatchObject({ kind: "html", mime: "text/html", reused: false, assets: ["img/chart.png"] });
    // The name is a keyed hash of the content, not the content hash.
    expect(first.object).toMatch(/^[0-9a-f]{64}\.[0-9a-f]{16}$/);
    expect(first.object.startsWith(first.hash)).toBe(false);
    expect(first.object).toBe(`${createHmac("sha256", transfers.nameSecret).update(first.hash).digest("hex")}.${first.object.split(".")[1]}`);
    expect(first.hash).toBe(createHash("sha256").update(open(backend.objects.get(first.object)!, Buffer.from(first.key, "base64"), Buffer.from(first.nonce, "base64"))).digest("hex"));
    expect(backend.objects.get(first.object)!.length).toBe(sealedSize(first.size));
    const second = await prepare(transfers, "read", "report.html");
    expect(second.reused).toBe(true);
    expect(second.key).toBe(first.key);
    expect(transfers.uploads).toEqual([first.object]);
  });

  it("uploads again when the server lost the object", async () => {
    const transfers = new MockTransfers();
    const backend = transfers.backend as MemoryBackend;
    const first = await prepare(transfers, "read", "report.html");
    backend.objects.clear();
    const again = await prepare(transfers, "read", "report.html");
    expect(again.reused).toBe(false);
    expect(again.object).toBe(first.object);
    expect(transfers.uploads).toHaveLength(2);
  });

  it("opens files outside the project for a Control device only", async () => {
    const transfers = new MockTransfers();
    const control = await prepare(transfers, "control", OUTSIDE_TEXT_PATH);
    expect(control).toMatchObject({ kind: "text", name: "outside-note.txt" });
    expect(await codeOf(() => prepare(transfers, "read", OUTSIDE_TEXT_PATH))).toBe("forbidden");
    expect(await codeOf(() => prepare(transfers, "control", "/tmp/not-there.txt"))).toBe("not_found");
  });

  it("refuses the sensitive files for every device", async () => {
    const transfers = new MockTransfers();
    for (const permission of ["read", "control"] as const) {
      for (const path of [SECRET_PATH, "/example/alpha/.env", "/home/x/.ssh/id_rsa"]) {
        expect(await codeOf(() => prepare(transfers, permission, path)), `${permission} ${path}`).toBe("path_denied");
      }
    }
  });

  it("serves an asset only of an html or markdown file, and only an image", async () => {
    const transfers = new MockTransfers();
    expect(await prepare(transfers, "read", "report.html", "img/chart.png")).toMatchObject({ kind: "image", mime: "image/png" });
    expect(await codeOf(() => prepare(transfers, "read", "src/example.ts", "img/chart.png"))).toBe("forbidden");
    expect(await codeOf(() => prepare(transfers, "read", "report.html", "../secret.png"))).toBe("forbidden");
  });

  it("seals files over 4 MB in several chunks that open to the original", async () => {
    const transfers = new MockTransfers();
    const backend = transfers.backend as MemoryBackend;
    const binary = await prepare(transfers, "control", LARGE_BINARY_PATH);
    expect(binary.size).toBe(LARGE_BINARY_BYTES);
    expect(binary.kind).toBe("binary");
    const sealed = backend.objects.get(binary.object)!;
    expect(sealed.length).toBe(LARGE_BINARY_BYTES + 6 * 16);
    expect(createHash("sha256").update(open(sealed, Buffer.from(binary.key, "base64"), Buffer.from(binary.nonce, "base64"))).digest("hex")).toBe(binary.hash);
    const text = await prepare(transfers, "control", LARGE_TEXT_PATH);
    expect(text.size).toBeGreaterThan(4_000_000);
    expect(text.kind).toBe("text");
  });
});
