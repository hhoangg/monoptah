// `/`: restores the last place of the last used desktop, else goes to /desktops. Owner: WS-L (workstreams.md 3.1).
import { createFileRoute, redirect } from "@tanstack/react-router";
import { resolveLastPlace } from "@/data/lastPlace";

export const Route = createFileRoute("/")({
  beforeLoad: () => {
    // The resolver is WS-T's (features/sessions/lastPlace.ts, W-06); it registers itself in data/lastPlace.ts.
    const place = resolveLastPlace();
    if (place === null) throw redirect({ to: "/desktops", replace: true });
    // The place is a runtime path (checked to start with a single slash) that may carry ?search and #hash:
    // `href` parses it, `to` would treat it as a bare path.
    throw redirect({ href: place, replace: true });
  },
});
