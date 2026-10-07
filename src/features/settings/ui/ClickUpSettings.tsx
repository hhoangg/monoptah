import { useCallback, useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { clearInboxCache } from "../../inbox/model/githubTasks";
import {
  CLICKUP_CHANGE_EVENT,
  clickupConnected,
  disconnectClickUp,
  listClickUpSpaces,
  listClickUpWorkspaces,
  loadHiddenClickUpSpaceIds,
  notifyClickUpChange,
  reconcileHiddenClickUpSpaceIds,
  saveClickUpConfig,
  saveHiddenClickUpSpaceIds,
  type ClickUpSpace,
  type ClickUpStatus,
  type ClickUpWorkspace,
} from "../../inbox/model/clickup";

export function ClickUpSettings() {
  const [status, setStatus] = useState<ClickUpStatus | null>(null);
  const [token, setToken] = useState("");
  // Null until the typed token has been looked up. Only a token with more than
  // one workspace needs the picker, and it cannot be saved before one is chosen.
  const [workspaces, setWorkspaces] = useState<ClickUpWorkspace[] | null>(null);
  const [teamId, setTeamId] = useState("");
  const [checking, setChecking] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [spaces, setSpaces] = useState<ClickUpSpace[]>([]);
  const [hiddenIds, setHiddenIds] = useState(loadHiddenClickUpSpaceIds);

  const loadSpaces = useCallback(async () => {
    try {
      setSpaces(await listClickUpSpaces());
    } catch (err) {
      setSpaces([]);
      setError(String(err instanceof Error ? err.message : err));
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void clickupConnected()
      .then(async (next) => {
        if (cancelled) return;
        setStatus(next);
        if (next.connected) await loadSpaces();
      })
      .catch((err: unknown) => {
        if (!cancelled)
          setError(String(err instanceof Error ? err.message : err));
      })
      .finally(() => {
        if (!cancelled) setChecking(false);
      });
    const onChange = () => setHiddenIds(loadHiddenClickUpSpaceIds());
    window.addEventListener(CLICKUP_CHANGE_EVENT, onChange);
    return () => {
      cancelled = true;
      window.removeEventListener(CLICKUP_CHANGE_EVENT, onChange);
    };
  }, [loadSpaces]);

  const editToken = (value: string) => {
    setToken(value);
    setWorkspaces(null);
    setTeamId("");
  };

  const connect = async () => {
    if (busy || checking || !token.trim()) return;
    setBusy(true);
    setError(null);
    try {
      let found = workspaces;
      if (!found) {
        found = await listClickUpWorkspaces(token);
        setWorkspaces(found);
      }
      if (found.length > 1 && !teamId) return;
      setStatus(
        await saveClickUpConfig({
          token,
          teamId: found.length > 1 ? teamId : null,
        }),
      );
      setToken("");
      setWorkspaces(null);
      setTeamId("");
      clearInboxCache();
      saveHiddenClickUpSpaceIds([]);
      await loadSpaces();
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    } finally {
      setBusy(false);
    }
  };

  const disconnect = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(await disconnectClickUp());
      setSpaces([]);
      setToken("");
      setWorkspaces(null);
      setTeamId("");
      clearInboxCache();
      notifyClickUpChange();
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    } finally {
      setBusy(false);
    }
  };

  const needsWorkspace = workspaces !== null && workspaces.length > 1;

  return (
    <div className="px-4 py-3.5">
      {checking ? (
        <p className="text-[12px] text-content/45">
          Checking ClickUp connection…
        </p>
      ) : status?.connected ? (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0 text-[12px] text-content/65">
            <p className="break-all">{status.teamName}</p>
            <p className="break-all">{status.username}</p>
          </div>
          <SecondaryButton onClick={() => void disconnect()} disabled={busy}>
            {busy ? "Disconnecting" : "Disconnect"}
          </SecondaryButton>
        </div>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void connect();
          }}
          className="flex flex-col gap-3"
        >
          <p className="text-[12px] leading-relaxed text-content/45">
            Connect ClickUp with a personal API token. Disconnect deletes the
            saved token.
          </p>
          <label className="flex flex-col gap-1 text-[12px] text-content/65">
            ClickUp API token
            <input
              aria-label="ClickUp API token"
              type="password"
              value={token}
              onChange={(event) => editToken(event.target.value)}
              placeholder="pk_..."
              disabled={busy}
              required
              autoComplete="off"
              spellCheck={false}
              className="h-8 w-full rounded-md border border-content/10 bg-transparent px-2 text-content outline-none focus:border-content/20"
            />
          </label>
          {needsWorkspace ? (
            <label className="flex flex-col gap-1 text-[12px] text-content/65">
              ClickUp workspace
              <select
                aria-label="ClickUp workspace"
                value={teamId}
                onChange={(event) => setTeamId(event.target.value)}
                disabled={busy}
                className="h-8 w-full rounded-md border border-content/10 bg-background-base px-2 text-content outline-none focus:border-content/20"
              >
                <option value="">Choose a workspace</option>
                {workspaces.map((workspace) => (
                  <option key={workspace.id} value={workspace.id}>
                    {workspace.name}
                  </option>
                ))}
              </select>
              <span className="text-content/45">
                This token reaches several workspaces. Pick one, then select
                Connect.
              </span>
            </label>
          ) : null}
          <div className="flex items-center gap-3">
            <SecondaryButton
              type="submit"
              disabled={busy || !token.trim() || (needsWorkspace && !teamId)}
            >
              {busy ? "Connecting" : "Connect"}
            </SecondaryButton>
            <button
              type="button"
              onClick={() =>
                void openUrl("https://app.clickup.com/settings/apps")
              }
              className="text-[12px] text-content/65 hover:text-content"
            >
              Create API token
            </button>
          </div>
        </form>
      )}
      {error ? (
        <p role="alert" className="mt-3 text-[12px] text-red-400/90">
          {error}
        </p>
      ) : null}
      {status?.connected ? (
        <div className="mt-4 flex flex-col gap-2">
          <div className="flex items-center justify-between">
            <span className="text-[13px] font-medium text-content">Spaces</span>
            <SecondaryButton
              disabled={busy || checking}
              onClick={() => void loadSpaces()}
            >
              Refresh spaces
            </SecondaryButton>
          </div>
          <p className="text-[12px] text-content/45">
            Unchecked spaces stay out of the inbox. &quot;Assigned to me&quot;
            looks at the newest 100 workspace tasks.
          </p>
          {spaces.map((space) => (
            <label
              key={space.id}
              className="flex items-center gap-2 text-[13px] text-content"
            >
              <input
                type="checkbox"
                checked={!hiddenIds.includes(space.id)}
                disabled={busy}
                onChange={() => {
                  const next = hiddenIds.includes(space.id)
                    ? hiddenIds.filter((id) => id !== space.id)
                    : [...hiddenIds, space.id];
                  clearInboxCache();
                  // Only ids from the live list are stored: the backend
                  // rejects a space id it cannot use.
                  saveHiddenClickUpSpaceIds(
                    reconcileHiddenClickUpSpaceIds(spaces, next),
                  );
                }}
              />
              {space.name}
            </label>
          ))}
        </div>
      ) : null}
    </div>
  );
}
