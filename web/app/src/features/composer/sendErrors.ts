// What the user reads when the desktop refuses or fails a send. `code` is the additive `code` of the desktop
// failure (protocol `DesktopErrorCode`), kept by the data layer as the `error` of a "Not sent" turn.
// An unknown code (or a transport error text) gets the generic line, never the raw code.
export const SEND_FAILED_MESSAGE = "The message could not be sent. Try again.";

const MESSAGES: Readonly<Record<string, string>> = {
  starting: "The cloud instance is still starting. Try again in a moment.",
  auto_stopping: "The cloud instance is stopping to save cost. Wait until it stops, then send again to wake it.",
  busy: "The desktop is busy with another request. Try again in a moment.",
  read_only: "This device can only read on this desktop. Use a device with control access to send.",
  not_selected: "This project is not shared with this device. Choose it in the desktop's Web app settings.",
  path_denied: "This device is not allowed to use that path.",
  wake_required: "The cloud instance is stopped. Confirm the wake-up to send this message.",
  instance_busy: "The cloud instance is busy. Try again in a moment.",
  forbidden: "This device is not allowed to send here.",
  revoked: "This device was removed from the desktop.",
  disabled: "Sending from the web is turned off on the desktop.",
  too_large: "The message is too large to send.",
  unsupported: "Update Monoptah on your desktop to send this.",
  unsupported_target: "This session cannot be sent to from the web.",
  not_pending: "This request is no longer waiting.",
  needs_desktop: "Answer this one on the desktop. It cannot be allowed once from here.",
  not_found: "That could not be found on your desktop.",
  fresh_required: "Confirm with your passkey first, then try again.",
  rate_limited: "Too many attempts. Wait a little and try again.",
  nothing_to_retry: "There is nothing to send again.",
};

/** The message for a send failure code; the generic line when the code is not known. */
export function sendErrorMessage(code: string | undefined): string {
  return (code && Object.hasOwn(MESSAGES, code) ? MESSAGES[code] : undefined) ?? SEND_FAILED_MESSAGE;
}
