// Minimal CBOR encoder for the virtual authenticator: unsigned and negative integers (up to 2^32), byte strings,
// text strings and maps (a `Map` keeps insertion order). Nothing else is needed for a "none" attestation object
// and a COSE EC2 key.
export type CborValue = number | string | Uint8Array | Map<number | string, CborValue>;

function head(major: number, value: number): number[] {
  if (value < 24) return [(major << 5) | value];
  if (value < 0x100) return [(major << 5) | 24, value];
  if (value < 0x10000) return [(major << 5) | 25, value >> 8, value & 0xff];
  if (value < 0x100000000) return [(major << 5) | 26, (value >>> 24) & 0xff, (value >>> 16) & 0xff, (value >>> 8) & 0xff, value & 0xff];
  throw new Error("CBOR integer too large for this encoder");
}

export function encodeCbor(value: CborValue): Uint8Array {
  const out: number[] = [];
  const write = (v: CborValue): void => {
    if (typeof v === "number") {
      if (!Number.isInteger(v)) throw new Error("CBOR encoder takes integers only");
      out.push(...(v >= 0 ? head(0, v) : head(1, -1 - v)));
    } else if (typeof v === "string") {
      const bytes = new TextEncoder().encode(v);
      out.push(...head(3, bytes.length), ...bytes);
    } else if (v instanceof Uint8Array) {
      out.push(...head(2, v.length), ...v);
    } else {
      out.push(...head(5, v.size));
      for (const [key, item] of v) {
        write(key);
        write(item);
      }
    }
  };
  write(value);
  return new Uint8Array(out);
}
