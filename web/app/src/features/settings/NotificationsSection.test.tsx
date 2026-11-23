import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { setApiTransport } from "@/features/accounts/api";
import { NotificationsSection } from "./NotificationsSection";
import type { PushPrefs } from "@/features/accounts/queries";
import { pushPrefsKey } from "./pushPrefs";

let restore: (() => void) | undefined;
afterEach(() => restore?.());

function mount(seed?: PushPrefs) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: 0 } } });
  if (seed) queryClient.setQueryData(pushPrefsKey("d1"), seed);
  const transport = vi.fn(async () => Response.json({ error: "Boom", code: "internal" }, { status: 500 }));
  restore = setApiTransport(transport);
  render(
    <QueryClientProvider client={queryClient}>
      <NotificationsSection desktopId="d1" />
    </QueryClientProvider>,
  );
  return transport;
}

describe("NotificationsSection load errors", () => {
  it("shows the error when there are no prefs to show", async () => {
    mount();
    expect(await screen.findByRole("alert")).toBeInTheDocument();
  });

  it("keeps showing cached prefs, without an error, when the refresh fails", async () => {
    const transport = mount({ enabled: false, categories: { agentFinished: true, agentInput: true, monoMessages: true } });
    await waitFor(() => expect(transport).toHaveBeenCalledTimes(1));
    // Let the failed answer land in the query before asserting that it is not shown.
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("switch", { name: "Notifications from this desktop" })).not.toBeChecked();
    expect(screen.getByRole("switch", { name: "Notifications from this desktop" })).toBeEnabled();
  });
});
