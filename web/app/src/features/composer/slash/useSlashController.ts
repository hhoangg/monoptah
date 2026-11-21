// The state behind the `/` menu of one composer: the rows the desktop offers, the mode that is on (Plan,
// Operator, Orchestrator), the command that waits for the next send (Draft, Side question), and what choosing a
// row does. It works for an open session (`conversationId`: each command is a `commands.run` call, so the desktop
// switches the mode or saves the draft itself) and for the new-session page (`projectId`: nothing exists yet, so
// Plan, Orchestrator and Draft become the `/plan `, `/orchestrator ` and `/draft ` prefix the desktop's own composer
// understands). It returns `null` when the desktop does not advertise `slashCommands`, so an older desktop shows no
// menu at all.
import { useCallback, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { SlashCommand } from "@monoptah/protocol";
import { linkKey, useLinkMutation, useLinkQuery } from "@/data";
import { useLinkDesktopId } from "@/link";
import { toast } from "@/ui/sonner";
import { useCapability } from "@/version/capabilities";

export type SlashModeId = "plan" | "operator" | "orchestrator";
export type SlashArmedId = "draft" | "btw";

const MODES: readonly string[] = ["plan", "operator", "orchestrator"];

export interface SlashController {
  /** Every row the desktop offers (the menu filters them). Empty while they load. */
  commands: readonly SlashCommand[];
  loading: boolean;
  /** The list could not be read: the menu says so instead of staying empty. */
  failed: boolean;
  /** Reads the rows again: called when the menu opens. */
  refresh: () => void;
  /** The mode that is on for the next message, with the row to show in its chip. */
  mode: SlashCommand | null;
  /** The command that takes the next message instead of a send: Draft, or a side question. */
  armed: SlashCommand | null;
  /** Runs a row the user chose (a skill needs nothing: the composer inserts its text). Rejects with a sentence to show. */
  choose: (row: SlashCommand) => Promise<void>;
  clearMode: () => Promise<void>;
  disarm: () => void;
  /** Sends the armed command with the message. Returns false when the page sends it itself (new session). */
  submitArmed: (text: string) => Promise<boolean>;
  /** The text a new session starts with: the mode or draft prefix in front of the message. */
  prefix: (text: string) => string;
  /** Call after a message was sent: the desktop cleared its mode. */
  afterSend: () => void;
  /** The page opens its own "Add to folder" dialog. */
  addToFolder: boolean;
}

export interface SlashControllerOptions {
  conversationId?: string;
  projectId?: string;
  harness?: string;
  readOnly?: boolean;
  /** Opens the folder dialog of the session (open sessions only). */
  onAddToFolder?: () => void;
}

function errorText(reason: unknown, fallback: string): Error {
  return new Error(reason instanceof Error && reason.message ? reason.message : fallback);
}

export function useSlashController({ conversationId, projectId, harness, readOnly = false, onAddToFolder }: SlashControllerOptions): SlashController | null {
  const supported = useCapability("slashCommands");
  const desktopId = useLinkDesktopId();
  const queryClient = useQueryClient();
  const params = useMemo(() => (conversationId ? { conversationId } : { ...(projectId ? { projectId } : {}), ...(harness ? { harness: harness as never } : {}) }), [conversationId, projectId, harness]);
  const enabled = supported && !readOnly && (conversationId !== undefined || projectId !== undefined);
  const query = useLinkQuery("commands.list", params, { enabled });
  const run = useLinkMutation("commands.run");
  // The mode and the armed command as the user set them here. `null` for the mode means "off"; `undefined` means the
  // desktop's own state (the rows) is the truth.
  const [localMode, setLocalMode] = useState<string | null | undefined>(undefined);
  const [armedId, setArmedId] = useState<SlashArmedId | null>(null);
  const commands = useMemo(() => query.data?.commands ?? [], [query.data]);
  const { refetch } = query;
  const refresh = useCallback(() => {
    void refetch();
  }, [refetch]);

  const modeId = localMode !== undefined ? localMode : (commands.find((row) => row.mode && row.active && MODES.includes(row.id))?.id ?? null);
  // The desktop's own mode changed (another device, the desktop itself): what it reports is the truth again.
  const desktopMode = commands.find((row) => row.mode && row.active && MODES.includes(row.id))?.id ?? null;
  const [seenDesktopMode, setSeenDesktopMode] = useState(desktopMode);
  if (seenDesktopMode !== desktopMode) {
    setSeenDesktopMode(desktopMode);
    setLocalMode(undefined);
  }
  const mode = modeId ? (commands.find((row) => row.id === modeId) ?? null) : null;
  const armed = armedId ? (commands.find((row) => row.id === armedId) ?? null) : null;

  const invalidate = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: linkKey(desktopId, "commands.list", params) });
  }, [queryClient, desktopId, params]);

  const choose = useCallback(
    async (row: SlashCommand) => {
      if (!row.available) throw new Error(row.reason ?? "This command is not available now.");
      if (row.kind === "skill") return;
      if (row.id === "add-to-folder") {
        onAddToFolder?.();
        return;
      }
      if (row.id === "draft" || row.id === "btw") {
        // Without a session the mode would ride on the same first message as the draft prefix.
        if (!conversationId) setLocalMode(null);
        setArmedId(row.id);
        return;
      }
      if (MODES.includes(row.id)) {
        if (!conversationId) {
          // A new session has nothing to switch yet: the mode rides on the first message.
          setArmedId(null);
          setLocalMode(modeId === row.id ? null : row.id);
          return;
        }
        try {
          const answer = await run.mutateAsync({ conversationId, command: row.id as SlashModeId, enabled: modeId !== row.id });
          setLocalMode(answer.active ? row.id : null);
          setArmedId(null);
          invalidate();
        } catch (reason) {
          throw errorText(reason, "Could not change the mode.");
        }
        return;
      }
      if (row.id === "compact" && conversationId) {
        try {
          await run.mutateAsync({ conversationId, command: "compact" });
          invalidate();
        } catch (reason) {
          throw errorText(reason, "Could not compact the context.");
        }
        return;
      }
    },
    [conversationId, modeId, onAddToFolder, run, invalidate],
  );

  const clearMode = useCallback(async () => {
    if (!modeId) return;
    if (conversationId) {
      try {
        await run.mutateAsync({ conversationId, command: modeId as SlashModeId, enabled: false });
      } catch (reason) {
        throw errorText(reason, "Could not change the mode.");
      }
      invalidate();
    }
    setLocalMode(null);
  }, [conversationId, modeId, run, invalidate]);

  const submitArmed = useCallback(
    async (text: string) => {
      if (!armedId || !conversationId) return false;
      try {
        const answer = await run.mutateAsync({ conversationId, command: armedId, text });
        // A side question is answered on the desktop: say so, or the message just vanishes.
        if (armedId === "btw") toast(answer.message ?? "The side question opened on the desktop.");
      } catch (reason) {
        throw errorText(reason, armedId === "draft" ? "Could not save the draft." : "Could not ask the side question.");
      }
      setArmedId(null);
      invalidate();
      return true;
    },
    [armedId, conversationId, run, invalidate],
  );

  const prefix = useCallback(
    (text: string) => {
      if (conversationId) return text;
      const word = armedId === "draft" ? "draft" : modeId === "plan" || modeId === "orchestrator" ? modeId : null;
      return word ? `/${word} ${text}` : text;
    },
    [conversationId, armedId, modeId],
  );

  const afterSend = useCallback(() => {
    setLocalMode(conversationId ? undefined : null);
    setArmedId(null);
    if (conversationId) invalidate();
  }, [conversationId, invalidate]);

  if (!supported) return null;
  return { commands, loading: query.isFetching && commands.length === 0, failed: query.isError && commands.length === 0, refresh, mode, armed, choose, clearMode, disarm: () => setArmedId(null), submitArmed, prefix, afterSend, addToFolder: onAddToFolder !== undefined };
}
