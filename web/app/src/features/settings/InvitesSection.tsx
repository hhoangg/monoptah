import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { InvitesResponse } from "@monoptah/protocol";
import { isApiError } from "@/features/accounts/api";
import { createInvite, invitesQuery, revokeInvite } from "@/features/accounts/queries";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { settingsErrorMessage } from "./copy";
import { formatDate } from "./format";
import { QueryBoundary } from "./QueryBoundary";
import { useAction } from "./useAction";

type Invite = ReturnType<typeof InvitesResponse.parse>["invites"][number];

/** The link a new user opens. The code is in the fragment, so it never reaches a server log. */
export function inviteLink(code: string, origin = window.location.origin): string {
  return `${origin}/register#invite=${encodeURIComponent(code)}`;
}

type InviteState = "unused" | "joined" | "revoked" | "expired";

function stateOf(invite: Invite, now: number): InviteState {
  if (invite.usedAt !== null) return "joined";
  if (invite.revokedAt !== null) return "revoked";
  if (invite.expiresAt <= now) return "expired";
  return "unused";
}

/** Invites (N-06): quota, create (code shown once), copy link, revoke, who joined. */
export function InvitesSection() {
  const query = useQuery(invitesQuery);
  return (
    <SettingsCard title="Invites" footnote="Each invite works once and expires after 7 days. Only people you invite can create an account.">
      <QueryBoundary query={query} label="invites">
        {(data) => <InviteList data={data} />}
      </QueryBoundary>
    </SettingsCard>
  );
}

function InviteList({ data }: { data: ReturnType<typeof InvitesResponse.parse> }) {
  const queryClient = useQueryClient();
  const revoke = useAction();
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  const [paused, setPaused] = useState(false);
  // The plain code exists only here, until the user taps Done or leaves. Never stored or cached.
  const [created, setCreated] = useState<{ code: string } | null>(null);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const copyTimer = useRef<number | undefined>(undefined);
  const [now] = useState(() => Math.floor(Date.now() / 1000));

  useEffect(
    () => () => {
      window.clearTimeout(copyTimer.current);
    },
    [],
  );

  async function onCreate() {
    setCreating(true);
    setCreateError(null);
    setCopyState("idle");
    try {
      const result = await createInvite(queryClient);
      setCreated({ code: result.code });
    } catch (caught) {
      if (isApiError(caught, "invites_paused")) setPaused(true);
      else setCreateError(settingsErrorMessage(caught));
    } finally {
      setCreating(false);
    }
  }

  async function onCopy(code: string) {
    window.clearTimeout(copyTimer.current);
    try {
      await navigator.clipboard.writeText(inviteLink(code));
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
    copyTimer.current = window.setTimeout(() => setCopyState("idle"), 2500);
  }

  const invites = [...data.invites].sort((a, b) => b.createdAt - a.createdAt);
  const noneLeft = data.remaining <= 0;

  return (
    <>
      <SettingsRow
        icon="userAdd"
        title="Invites left"
        description={`${data.remaining} of ${data.granted} available`}
        trailing={
          <Button type="button" variant="outline" size="sm" disabled={creating || noneLeft || paused} aria-busy={creating} onClick={() => void onCreate()}>
            Create invite
          </Button>
        }
      />
      {paused ? (
        <div className="px-1.5 py-2">
          <Notice variant="info" title="Invites are paused">
            New accounts cannot be invited right now. Try again later.
          </Notice>
        </div>
      ) : null}
      {createError ? (
        <div className="px-1.5 py-2">
          <Notice variant="error">{createError}</Notice>
        </div>
      ) : null}
      {created ? (
        <div className="m-1.5 flex flex-col gap-3 rounded-row bg-fill p-3.5" data-testid="new-invite">
          <p className="text-body font-semibold text-content">Your new invite</p>
          <p className="text-small text-muted-text">Copy this link now and send it to the person you invite. Monoptah cannot show it again.</p>
          <p className="rounded-row border border-stroke bg-background p-3 font-mono text-small break-all text-content select-all">{inviteLink(created.code)}</p>
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" onClick={() => void onCopy(created.code)}>
              <Icon name={copyState === "copied" ? "check" : "copy"} size={18} />
              {copyState === "copied" ? "Copied" : "Copy link"}
            </Button>
            <Button type="button" variant="ghost" onClick={() => setCreated(null)}>
              Done
            </Button>
          </div>
          <p role="status" className="text-meta text-muted-text">
            {copyState === "failed" ? "Could not copy. Select the link and copy it by hand." : ""}
          </p>
        </div>
      ) : null}
      {revoke.error ? (
        <div className="px-1.5 py-2">
          <Notice variant="error">{revoke.error}</Notice>
        </div>
      ) : null}
      {invites.length > 0 ? (
        <ul className="m-0 list-none p-0">
          {invites.map((invite) => {
            const state = stateOf(invite, now);
            const title = state === "joined" ? `${invite.usedBy ?? "Someone"} joined` : state === "unused" ? "Unused invite" : state === "revoked" ? "Revoked invite" : "Expired invite";
            const description =
              state === "joined"
                ? `Created ${formatDate(invite.createdAt)}, joined ${formatDate(invite.usedAt ?? invite.createdAt)}`
                : state === "unused"
                  ? `Created ${formatDate(invite.createdAt)}, expires ${formatDate(invite.expiresAt)}`
                  : `Created ${formatDate(invite.createdAt)}`;
            return (
              <SettingsRow
                  as="li"
                  key={invite.id}
                  icon={state === "joined" ? "success" : "link"}
                  title={title}
                  description={description}
                  trailing={
                    state === "unused" ? (
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        disabled={revoke.busy}
                        aria-label={`Revoke the invite created ${formatDate(invite.createdAt)}`}
                        onClick={() => void revoke.run(() => revokeInvite(queryClient, invite.id))}
                      >
                        Revoke
                      </Button>
                    ) : null
                  }
                />
            );
          })}
        </ul>
      ) : (
        <p className="px-1.5 py-3 text-small text-muted-text">You have not created any invites yet.</p>
      )}
    </>
  );
}
