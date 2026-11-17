import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { SessionsResponse } from "@monoptah/protocol";
import { revokeSession, sessionsQuery } from "@/features/accounts/queries";
import { Badge } from "@/ui/badge";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { describeUserAgent, formatRelative } from "./format";
import { QueryBoundary } from "./QueryBoundary";
import { useAction } from "./useAction";

type Session = ReturnType<typeof SessionsResponse.parse>["sessions"][number];

function details(session: Session): string {
  const used = `last used ${formatRelative(session.lastSeenAt)}`;
  return session.amr === "recovery" ? `Signed in with a recovery code, ${used}` : `Signed in with a passkey, ${used}`;
}

/** Signed-in browsers (N-05). The contract has no bulk route: "everywhere else" revokes each other session. */
export function SessionsSection() {
  const query = useQuery(sessionsQuery);
  return (
    <SettingsCard title="Signed-in browsers" footnote="Sign out a browser you no longer use or do not recognise. It has to sign in again with a passkey.">
      <QueryBoundary query={query} label="signed-in browsers">
        {(data) => <SessionList sessions={[...data.sessions].sort((a, b) => Number(b.current) - Number(a.current) || b.lastSeenAt - a.lastSeenAt)} />}
      </QueryBoundary>
    </SettingsCard>
  );
}

function SessionList({ sessions }: { sessions: Session[] }) {
  const queryClient = useQueryClient();
  const one = useAction();
  const all = useAction();
  const [confirmAll, setConfirmAll] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const others = sessions.filter((session) => !session.current);

  async function signOutOne(session: Session) {
    setPending(session.id);
    await one.run(() => revokeSession(queryClient, session.id));
    setPending(null);
  }

  async function signOutOthers() {
    const ok = await all.run(async () => {
      // One call per browser; revokeSession refreshes the list after each.
      await Promise.all(others.map((session) => revokeSession(queryClient, session.id)));
    });
    if (ok) setConfirmAll(false);
  }

  return (
    <>
      <ul className="m-0 list-none p-0">
        {sessions.map((session) => (
          <li key={session.id}>
            <SettingsRow
              icon="computer"
              title={
                <span className="flex flex-wrap items-center gap-2">
                  {describeUserAgent(session.userAgent)}
                  {session.current ? <Badge variant="outline">This browser</Badge> : null}
                </span>
              }
              description={details(session)}
              trailing={
                session.current ? null : (
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={pending === session.id}
                    aria-label={`Sign out ${describeUserAgent(session.userAgent)}, last used ${formatRelative(session.lastSeenAt)}`}
                    onClick={() => void signOutOne(session)}
                  >
                    Sign out
                  </Button>
                )
              }
            />
          </li>
        ))}
      </ul>
      <div className="flex flex-col gap-3 px-1.5 py-2">
        {one.error ? <Notice variant="error">{one.error}</Notice> : null}
        <Button
          type="button"
          variant="outline"
          className="w-full sm:w-auto sm:self-start"
          disabled={others.length === 0}
          onClick={() => {
            all.reset();
            setConfirmAll(true);
          }}
        >
          Sign out everywhere else
        </Button>
        {others.length === 0 ? <p className="text-meta text-muted-text">This is the only browser signed in.</p> : null}
      </div>
      <ConfirmDialog
        open={confirmAll}
        onOpenChange={setConfirmAll}
        title="Sign out everywhere else?"
        description={`${others.length} other ${others.length === 1 ? "browser" : "browsers"} will be signed out. This browser stays signed in.`}
        confirmLabel="Sign out everywhere else"
        destructive
        busy={all.busy}
        error={all.error}
        onConfirm={() => void signOutOthers()}
      />
    </>
  );
}
