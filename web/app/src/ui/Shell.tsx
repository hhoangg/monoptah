import { Link, Outlet } from "@tanstack/react-router";
import { Glass } from "./Glass";
import { Icon, type IconName } from "./Icon";

const NAV: ReadonlyArray<{ to: string; label: string; icon: IconName }> = [
  { to: "/desktops", label: "Desktops", icon: "computer" },
  { to: "/settings", label: "Settings", icon: "settings" },
  { to: "/install", label: "Install", icon: "download" },
];

/** Shared Wave 0 shell: 44px header on a glass pill, safe-area padding, one scrolling main. */
export function Shell() {
  return (
    <div className="flex min-h-dvh flex-col bg-background text-content">
      <header className="sticky top-0 z-10 flex min-h-11 items-center gap-2 px-5 pt-[env(safe-area-inset-top)]">
        <Glass className="flex min-h-11 flex-1 items-center gap-1 px-2">
          <Link to="/" className="rounded-pill px-3 py-2 text-small font-semibold">
            Monoptah
          </Link>
          <nav aria-label="Main" className="ml-auto flex items-center gap-1">
            {NAV.map((item) => (
              <Link
                key={item.to}
                to={item.to}
                aria-label={item.label}
                className="inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9 [&.active]:text-content"
              >
                <Icon name={item.icon} />
              </Link>
            ))}
          </nav>
        </Glass>
      </header>
      <main className="flex-1 pb-[calc(env(safe-area-inset-bottom)+2.5rem)]">
        <Outlet />
      </main>
    </div>
  );
}
