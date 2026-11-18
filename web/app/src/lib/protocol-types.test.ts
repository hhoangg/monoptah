import { describe, expect, expectTypeOf, it } from "vitest";
import { findRoute, type MeResponse, type PatchMeBody, type User } from "@monoptah/protocol";

// The freeze rule of workstreams.md 2 ("the web client compiles against the protocol"): the app
// depends on @monoptah/protocol and types a route through the API registry. `pnpm typecheck` fails
// here if the registry or a schema changes shape under the client.
type Output<T extends { _output: unknown }> = T["_output"];

describe("the app compiles against @monoptah/protocol", () => {
  it("types the GET /api/me response through the API registry", () => {
    const route = findRoute("GET", "/api/me");
    expect(route?.auth).toBe("session");
    const me: Output<typeof MeResponse> = {
      user: { id: "u1", handle: "alice", displayName: "Alice", role: "user" },
      invitesLeft: 3,
      recoveryOnly: false,
      recoveryCodesLeft: 10,
      recoveryAckAt: null,
    };
    expect(route?.response.safeParse(me).success).toBe(true);
    expectTypeOf(me.user).toEqualTypeOf<User>();
    expectTypeOf(me.recoveryAckAt).toEqualTypeOf<number | null>();
  });

  it("types a request body", () => {
    const body: Output<typeof PatchMeBody> = { displayName: "Alice" };
    expect(findRoute("PATCH", "/api/me")?.request.body?.safeParse(body).success).toBe(true);
  });
});
