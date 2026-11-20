// What the user reads about a terminal: why a session ended and why an open or a write failed.
// Exit reasons are the `exited.reason` of `terminal.read` (protocol TERMINAL_EXIT_REASONS) plus two the web adds itself.
import { ReauthCancelledError } from "@/features/accounts/queries";

/** Reasons the web adds: the desktop forgot the terminal, or the link failed for good. */
export const LOCAL_EXIT_REASONS = ["gone", "error"] as const;

export interface EndedText {
  reason: string;
  code?: number | undefined;
  /** Minutes of the idle limit, for the `idle` text. */
  idleMinutes?: number | undefined;
}

function minutes(value: number | undefined): string {
  return value === undefined || value <= 0 ? "a while" : `${value} minute${value === 1 ? "" : "s"}`;
}

/** "Session ended: <this>". Never echoes a reason string the desktop sent that this build does not know. */
export function endedText({ reason, code, idleMinutes }: EndedText): string {
  switch (reason) {
    case "exit":
      return code === undefined ? "the shell exited." : `the shell exited with code ${code}.`;
    case "closed":
      return "you closed the terminal.";
    case "revoked":
      return "this device was removed from the desktop.";
    case "idle":
      return `the terminal was idle for ${minutes(idleMinutes)}.`;
    case "max":
      return "the terminal reached its time limit.";
    case "toggle_off":
      return "Allow web terminal was turned off for this device on the desktop.";
    case "killed":
      return "the terminal was closed on the desktop.";
    case "grace":
      return "the connection was lost for too long, so the desktop closed the terminal.";
    case "desktop":
      return "the desktop closed the terminal (it may have quit or restarted).";
    case "gone":
      return "the desktop no longer has this terminal.";
    case "error":
      return "the terminal could not be read any more.";
    default:
      return "the terminal ended.";
  }
}

const OPEN_FAILED: Readonly<Record<string, string>> = {
  read_only: "This device can only read on this desktop. Ask for control access to use a terminal.",
  forbidden: "This device is not allowed to open a terminal.",
  disabled: "Turn on Allow web terminal for this device in Monoptah on your desktop.",
  not_selected: "This project is not shared with this device. Choose it in the desktop's Web app settings.",
  busy: "The desktop has too many terminals open. Close one and try again.",
  fresh_required: "The desktop refused the passkey check. Try again.",
  ticket_refused: "The desktop refused the passkey check. Try again.",
  not_found: "That project or folder could not be found on your desktop.",
  path_denied: "That folder is not allowed. Use a folder inside the project.",
  unsupported: "Update Monoptah on your desktop to use the terminal.",
  revoked: "This device was removed from the desktop.",
  not_ready: "The desktop is not connected right now. Try again when it is back.",
  session_ended: "The connection dropped. Try again.",
  timeout: "The desktop did not answer in time. Try again.",
};

export const OPEN_FAILED_FALLBACK = "The terminal could not be opened. Try again.";

/** The message of a failed `terminal.open` for the desktop's `code` (a link error code as well). */
export function openFailedText(code: string | undefined): string {
  return (code && Object.hasOwn(OPEN_FAILED, code) ? OPEN_FAILED[code] : undefined) ?? OPEN_FAILED_FALLBACK;
}

/** The message of a failed ticket request: a closed passkey prompt, or an API error (`forbidden`, `fresh_required`, ...). */
export function ticketFailedText(error: unknown): string {
  if (error instanceof ReauthCancelledError) return PASSKEY_CANCELLED_NOTICE;
  const code = typeof error === "object" && error !== null && "code" in error ? (error as { code?: unknown }).code : undefined;
  return openFailedText(typeof code === "string" ? code : undefined);
}

/** The passkey prompt was closed before it finished: nothing was opened. */
export const PASSKEY_CANCELLED_NOTICE = "Passkey confirmation was cancelled. No terminal was opened.";
export const NOT_ALLOWED_NOTICE = "Turn on Allow web terminal for this device in Monoptah on your desktop";

export const INPUT_DROPPED_NOTICE = "Too much input waiting: some of it was dropped.";
export const INPUT_LOST_NOTICE = "The connection dropped: the last keys may not have reached the shell.";

/** The label of the navigation entries (the footer chip and the tab titles). */
export const TERMINAL_NAV_LABEL = "Terminal";
