import { QueryClientProvider } from "@tanstack/react-query";
import { act, render, waitFor } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { createQueryClient } from "@/lib/queryClient";
import { LinkProvider, getLinkConnection, resetLinkConnections } from "@/link";
import { clearLinkStores } from "@/link/store.ts";
import { LINK_GC_TIME_MS, catalogKey, conversationKey, linkKey, linkQueryOptions, useLinkQuery } from "./queries.ts";
import { conversationQueryOptions } from "./conversation.ts";
import { useLinkMutation } from "./mutations.ts";
import { FakeClient, requestError } from "./testing.ts";

afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
});

describe("keys and defaults", () => {
  it("builds the keys of architecture.md 3.2", () => {
    expect(linkKey("dk1", "sessions.list", { projectId: "p" })).toEqual(["link", "dk1", "sessions.list", { projectId: "p" }]);
    expect(conversationKey("dk1", "c1")).toEqual(["link", "dk1", "conversation", "c1"]);
    expect(catalogKey("dk1")).toEqual(["link", "dk1", "workspace.list", {}]);
  });

  it("the factories set staleTime Infinity, networkMode always and gcTime 10 min", () => {
    for (const options of [linkQueryOptions("dk1", "workspace.list", {}), conversationQueryOptions("dk1", "c1")]) {
      expect(options).toMatchObject({ staleTime: Infinity, networkMode: "always", gcTime: 10 * 60_000 });
    }
    expect(LINK_GC_TIME_MS).toBe(600_000);
  });

  it("queryFn calls request with method and params and returns the result", async () => {
    const client = new FakeClient();
    client.request.mockResolvedValue({ projects: [], sessions: [] });
    const queryClient = createQueryClient();
    const options = linkQueryOptions("dk1", "workspace.list", {}, { connection: client.asClient() });
    expect(await queryClient.fetchQuery(options)).toEqual({ projects: [], sessions: [] });
    expect(client.request.mock.calls[0]?.slice(0, 2)).toEqual(["workspace.list", {}]);
  });

  it("a failed request is not retried by the query", async () => {
    const client = new FakeClient();
    client.request.mockRejectedValue(requestError("not_ready"));
    const queryClient = createQueryClient();
    await expect(queryClient.fetchQuery(linkQueryOptions("dk1", "workspace.list", {}, { connection: client.asClient() }))).rejects.toMatchObject({ code: "not_ready" });
    expect(client.request).toHaveBeenCalledTimes(1);
  });
});

const pending = (() => new Promise<Response>(() => undefined)) as typeof fetch;

describe("hooks", () => {
  it("useLinkQuery reads the desktop of the provider, and useLinkMutation does not retry", async () => {
    const connection = getLinkConnection("dk9", { fetch: pending, ensureDevice: async () => ({ deviceId: "d", publicKey: new Uint8Array(32), fingerprint: "f" }) });
    let calls = 0;
    connection.request = (async (method: string) => {
      calls += 1;
      if (method === "conversation.send") throw requestError("session_ended");
      return { projects: [], sessions: [] };
    }) as typeof connection.request;
    let send: ReturnType<typeof useLinkMutation<"conversation.send">> | undefined;
    function Probe() {
      const query = useLinkQuery("workspace.list", {});
      const mutation = useLinkMutation("conversation.send");
      useEffect(() => {
        send = mutation;
      });
      return <p data-testid="status">{query.status}</p>;
    }
    const queryClient = createQueryClient();
    const view = render(
      <QueryClientProvider client={queryClient}>
        <LinkProvider desktopId="dk9">
          <Probe />
        </LinkProvider>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(view.getByTestId("status")).toHaveTextContent("success"));
    expect(queryClient.getQueryData(["link", "dk9", "workspace.list", {}])).toEqual({ projects: [], sessions: [] });
    const before = calls;
    await act(async () => {
      await send!.mutateAsync({ conversationId: "c", text: "x" }).catch(() => undefined);
    });
    expect(calls - before).toBe(1);
  });
});
