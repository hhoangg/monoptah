import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { ipHash, ipKey } from "../src/lib/ip.ts";

describe("ipKey", () => {
  it("keeps an IPv4 address as is", () => {
    expect(ipKey("203.0.113.7")).toBe("203.0.113.7");
  });

  it("keys IPv6 on its /64 prefix", () => {
    expect(ipKey("2001:db8:1:2:aaaa:bbbb:cccc:dddd")).toBe("2001:db8:1:2::/64");
    expect(ipKey("2001:db8:1:2:1111:2222:3333:4444")).toBe("2001:db8:1:2::/64");
    expect(ipKey("2001:db8:1:3::1")).toBe("2001:db8:1:3::/64");
  });

  it("normalizes the textual form: case, leading zeros, the :: shorthand and a zone id", () => {
    const canonical = "2001:db8:0:5::/64";
    expect(ipKey("2001:DB8:0:5::1")).toBe(canonical);
    expect(ipKey("2001:0db8:0000:0005:0000:0000:0000:0001")).toBe(canonical);
    expect(ipKey("2001:db8::5:0:0:0:1")).toBe("2001:db8:0:5::/64");
    expect(ipKey("2001:db8:0:5::1%eth0")).toBe(canonical);
    expect(ipKey("2001:db8::")).toBe("2001:db8:0:0::/64");
    expect(ipKey("::1")).toBe("0:0:0:0::/64");
    expect(ipKey("::")).toBe("0:0:0:0::/64");
  });

  it("treats an IPv4-mapped address as the IPv4 address", () => {
    expect(ipKey("::ffff:203.0.113.7")).toBe("203.0.113.7");
    expect(ipKey("::FFFF:203.0.113.7")).toBe("203.0.113.7");
    expect(ipKey("0:0:0:0:0:ffff:203.0.113.7")).toBe("203.0.113.7");
    expect(ipKey("::ffff:cb00:7107")).toBe("203.0.113.7");
  });

  it("keeps text that is not an address (an unknown client) as lowercase text", () => {
    expect(ipKey("unknown")).toBe("unknown");
    expect(ipKey("zz::1::2")).toBe("zz::1::2");
  });
});

describe("ipHash", () => {
  it("is the same for every address of one /64 and for every spelling, different across prefixes", async () => {
    const a = await ipHash(env, "2001:db8:1:2:aaaa::1");
    expect(await ipHash(env, "2001:DB8:1:2::ffff")).toBe(a);
    expect(await ipHash(env, "2001:db8:1:3::1")).not.toBe(a);
    expect(await ipHash(env, "::ffff:198.51.100.9")).toBe(await ipHash(env, "198.51.100.9"));
    expect(await ipHash(env, "198.51.100.9")).not.toBe(await ipHash(env, "198.51.100.10"));
  });
});
