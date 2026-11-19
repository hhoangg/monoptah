import { defineModule } from "../define.ts";
import { listSessions, revokeSession } from "./handlers.ts";

export { logout } from "./handlers.ts";
export {
  ABSOLUTE_SECONDS,
  clearSessionCookie,
  closeSessionSockets,
  createSession,
  deleteAllSessionsStatement,
  deleteSessionStatement,
  FRESH_SECONDS,
  IDLE_SECONDS,
  listSessionIdHashes,
  publicSessionId,
  RECOVERY_ONLY_SECONDS,
  revokeAllSessions,
  revokeSessions,
  SESSION_COOKIE,
  sessionCookie,
  type CreatedSession,
  type CreateSessionInput,
} from "./session.ts";

/** Routes of the `sessions` module: list own sessions and revoke one. Logout is `POST /api/auth/logout` (auth module). */
export const routes = defineModule("sessions", {
  "GET /api/sessions": listSessions,
  "DELETE /api/sessions/:id": revokeSession,
});
