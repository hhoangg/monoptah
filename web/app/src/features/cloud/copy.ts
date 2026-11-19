import { WAKE_REQUIRED_NOTICE } from "@/ui/hooks";

// Cost notices and explanations of the cloud UI (architecture.md 6.6 billing rule). Estimates are labelled.
// Cloud text from the desktop is never rendered here: only these strings and the sanitized error.
export const READ_ONLY_NOTE = "This device has read-only access. Cloud actions need a device with control access.";

export const COST_NOTICES = {
  wake: WAKE_REQUIRED_NOTICE,
  wakeStopping: "This instance is stopping. Sending may wake it again, and a running instance is billable until it stops.",
  wakeUnknown: "The desktop cannot reach this instance. Sending may wake it, and a running instance is billable until it stops.",
  resume: "Resuming starts this stopped instance. A running instance is billable until it stops.",
  retry: "Retrying starts the instance setup again. The instance is billable while it runs.",
  start: "Starting a cloud session creates or starts an instance. A running instance is billable until it stops.",
  moveToCloud: "Moving this session to the cloud starts an instance. A running instance is billable until it stops.",
  moveToLocal: "Continuing locally may wake the cloud instance to copy the work back, which is billable while it runs.",
  archive: "Archiving may wake the instance to convert the session, which is billable while it runs.",
} as const;

export const STARTING_REFUSAL = "The instance is still starting. Wait for the steps to finish.";
export const UNAVAILABLE_REFUSAL = "This instance is unavailable right now. Check it on your desktop, then send again.";
export const STOPPING_NOTE = "This instance is stopping. Sending after it stops wakes it again (billable).";
export const UNAVAILABLE_NOTE = "This instance is unavailable right now. Sending is off until it is back.";
export const STOPPED_NOTE = "This instance is stopped. Sending wakes it (billable).";
export const WAKING_NOTE = "Waking the instance. Your message is queued and sends when it is ready.";
export const NOT_CONNECTED_NOTE = "Not connected. The desktop cannot reach this instance right now.";
export const KEEP_RUNNING_NOTE = "Keeping it running keeps billing until it stops.";
export const KEPT_NOTE = "The instance was kept after the failed start and still bills until it is stopped.";
export const STOP_CONFIRM = "Stopping interrupts any work running on the instance. The desktop tells you what was interrupted.";
export const CLOUD_NAV_LABEL = "Cloud";
export const CLAUDE_ONLY_NOTE = "Cloud sessions run Claude.";
export const START_DISABLED_NOTE = "Remote cloud start is turned off on your desktop. Turn it on in the desktop settings to start cloud sessions.";
export const MOVE_DISABLED_NOTE = "Remote cloud start or Remote push in Git is turned off on your desktop. Turn them on in the desktop settings to move sessions.";
export const CANCEL_MOVE_CONFIRM = "Cancelling stops the move. Your session and its workspace are kept as they are, and nothing is deleted.";
