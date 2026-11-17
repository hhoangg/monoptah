import type { ReactNode } from "react";

export interface RouteStubProps {
  /** The route path as written in architecture.md 3.1, for example `/d/$desktopId/s/$sessionId`. */
  route: string;
  /** One-line purpose from architecture.md 3.1. */
  purpose: string;
  /** Route params, rendered as plain React text (never as markup). */
  params?: Record<string, string>;
  children?: ReactNode;
}

/** Wave 0 placeholder every route renders until its workstream replaces it. */
export function RouteStub({ route, purpose, params = {}, children }: RouteStubProps) {
  const entries = Object.entries(params);
  return (
    <section data-testid="route-stub" data-route={route} className="mx-auto flex w-full max-w-[760px] flex-col gap-3 px-5 py-6">
      <p className="text-meta font-medium text-muted-text">Route stub</p>
      <h1 className="text-headline font-semibold text-content">{route}</h1>
      <p className="text-body text-muted-text">{purpose}</p>
      <dl className="flex flex-col gap-1.5 rounded-card bg-surface p-3.5 text-small">
        {entries.length === 0 ? (
          <dd className="text-muted-text">No params</dd>
        ) : (
          entries.map(([key, value]) => (
            <div key={key} className="flex gap-3">
              <dt className="w-28 shrink-0 text-muted-text">{key}</dt>
              <dd data-param={key} className="min-w-0 break-all font-mono text-content">
                {value}
              </dd>
            </div>
          ))
        )}
      </dl>
      {children}
    </section>
  );
}
