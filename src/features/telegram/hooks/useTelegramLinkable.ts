import { useEffect, useState } from "react";
import { telegramStatus } from "../model/telegram";

/**
 * Whether a project has a bot and a connected group, so a session of it can be
 * linked to a topic. Looked up when `project` is set, so a menu pays for it
 * only while it is open.
 */
export function useTelegramLinkable(project: string | null): boolean {
  const [linkable, setLinkable] = useState<{
    project: string;
    value: boolean;
  } | null>(null);

  useEffect(() => {
    if (!project) return;
    let disposed = false;
    void telegramStatus(project).then(
      (status) => {
        if (disposed) return;
        setLinkable({
          project,
          value: status.configured && status.chatId != null,
        });
      },
      () => {
        if (!disposed) setLinkable({ project, value: false });
      },
    );
    return () => {
      disposed = true;
    };
  }, [project]);

  return !!project && linkable?.project === project && linkable.value;
}
