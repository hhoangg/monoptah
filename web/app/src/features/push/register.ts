// Startup wiring of P-04, imported for its side effects by one line in src/main.tsx: a notification click on
// an open window arrives as a message from the service worker and goes through the router.
import { router } from "@/router";
import { installNavigationListener } from "./navigationMessage";

installNavigationListener((url) => {
  void router.navigate({ href: url });
});
