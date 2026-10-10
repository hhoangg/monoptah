import { sessionModelForHarness } from "../../features/sessions/model/models";
import {
  newDefaultSession,
  newSession,
  type HarnessId,
  type RuntimeMode,
} from "../../features/sessions/model/session";
import { worktreeFocus } from "../../features/source-control/model/worktreeFocus";
import { sameProjectPath } from "../../features/projects/model/recents";

/** Session defaults can supply permissions, but the visible project owns cwd. */
export function newWorkspaceSession(
  project: string,
  runtimeMode?: RuntimeMode,
  harness?: HarnessId,
) {
  const focus = worktreeFocus(project);
  return {
    ...(harness
      ? newSession(
          harness,
          project,
          sessionModelForHarness(harness, project),
          runtimeMode,
        )
      : newDefaultSession(project, runtimeMode)),
    ...(focus && !sameProjectPath(focus.path, project)
      ? { worktreeCwd: focus.path, branch: focus.branch ?? undefined }
      : {}),
  };
}
