// `/`: restores the last place of the last used desktop (N-12), else goes to /desktops. A desktop that is no
// longer in the account's list is dropped, so a revoked one is never opened. Owner: WS-L (workstreams.md 3.1).
import { createFileRoute, redirect } from "@tanstack/react-router";
import { resolveLastPlace } from "@/data/lastPlace";
import { desktopsListQuery } from "@/features/desktops/api";
import { forgetDesktop, placeDesktop } from "@/features/sessions/lastPlace";
import type { QueryClient } from "@tanstack/react-query";

/** False only when the account's desktop list loaded and does not hold `desktopId`; a failed load keeps the place. */
async function stillInAccount(queryClient: QueryClient, desktopId: string): Promise<boolean> {
  try {
    const { desktops } = await queryClient.fetchQuery({ ...desktopsListQuery, staleTime: 30_000 });
    return desktops.some((desktop) => desktop.id === desktopId);
  } catch {
    return true;
  }
}

export const Route = createFileRoute("/")({
  beforeLoad: async ({ context }) => {
    // The resolver is WS-T's (features/sessions/lastPlace.ts, W-06); it registers itself in data/lastPlace.ts.
    const place = resolveLastPlace();
    if (place === null) throw redirect({ to: "/desktops", replace: true });
    const desktopId = placeDesktop(place);
    if (desktopId && !(await stillInAccount(context.queryClient, desktopId))) {
      forgetDesktop(desktopId);
      throw redirect({ to: "/desktops", replace: true });
    }
    // The place is a runtime path (checked to start with a single slash) that may carry ?search and #hash:
    // `href` parses it, `to` would treat it as a bare path.
    throw redirect({ href: place, replace: true });
  },
});
