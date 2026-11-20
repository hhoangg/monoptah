// A terminal tab of a pane (from 640px): the session of its id in the pane body. The session outlives this component.
import { TerminalPane } from "./TerminalPane";
import { useTerminalSession } from "./sessions";

export function TerminalTab({ id }: { id: string }) {
  const session = useTerminalSession(id);
  return <TerminalPane session={session} variant="tab" />;
}
