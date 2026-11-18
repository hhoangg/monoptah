// Every `/api/*` route the account screens (W-02, W-03, W-10, W-11) call, typed once.
import {
  AddPasskeyOptionsResponse,
  AddPasskeyVerifyBody,
  CreateInviteResponse,
  DesktopsResponse,
  HealthResponse,
  InvitesResponse,
  LoginOptionsBody,
  LoginOptionsResponse,
  LoginVerifyBody,
  LoginVerifyResponse,
  MeResponse,
  PairApproveBody,
  PairApproveResponse,
  PairInfoResponse,
  PasskeyResponse,
  PasskeysResponse,
  PatchMeBody,
  PatchMeResponse,
  PushPrefsBody,
  PushPrefsResponse,
  ReauthOptionsResponse,
  ReauthVerifyBody,
  ReauthVerifyResponse,
  RecoverBody,
  RecoverResponse,
  RegenerateRecoveryResponse,
  RegisterOptionsBody,
  RegisterOptionsResponse,
  RegisterVerifyBody,
  RegisterVerifyResponse,
  RenamePasskeyBody,
  SessionsResponse,
} from "@monoptah/protocol";
import { endpoint } from "./api";

/** `{ ok: true }`, the body of the routes that only confirm. */
const Ok = HealthResponse;

export const endpoints = {
  registerOptions: endpoint("POST", "/api/auth/register/options", RegisterOptionsResponse, RegisterOptionsBody),
  registerVerify: endpoint("POST", "/api/auth/register/verify", RegisterVerifyResponse, RegisterVerifyBody),
  loginOptions: endpoint("POST", "/api/auth/login/options", LoginOptionsResponse, LoginOptionsBody),
  loginVerify: endpoint("POST", "/api/auth/login/verify", LoginVerifyResponse, LoginVerifyBody),
  reauthOptions: endpoint("POST", "/api/auth/reauth/options", ReauthOptionsResponse),
  reauthVerify: endpoint("POST", "/api/auth/reauth/verify", ReauthVerifyResponse, ReauthVerifyBody),
  recover: endpoint("POST", "/api/auth/recover", RecoverResponse, RecoverBody),
  logout: endpoint("POST", "/api/auth/logout", Ok),

  me: endpoint("GET", "/api/me", MeResponse),
  patchMe: endpoint("PATCH", "/api/me", PatchMeResponse, PatchMeBody),

  sessions: endpoint("GET", "/api/sessions", SessionsResponse),
  revokeSession: endpoint("DELETE", "/api/sessions/:id", Ok),

  passkeys: endpoint("GET", "/api/passkeys", PasskeysResponse),
  addPasskeyOptions: endpoint("POST", "/api/passkeys/options", AddPasskeyOptionsResponse),
  addPasskeyVerify: endpoint("POST", "/api/passkeys/verify", PasskeyResponse, AddPasskeyVerifyBody),
  renamePasskey: endpoint("PATCH", "/api/passkeys/:id", PasskeyResponse, RenamePasskeyBody),
  deletePasskey: endpoint("DELETE", "/api/passkeys/:id", Ok),

  regenerateRecoveryCodes: endpoint("POST", "/api/recovery-codes/regenerate", RegenerateRecoveryResponse),
  ackRecoveryCodes: endpoint("POST", "/api/recovery-codes/ack", Ok),

  invites: endpoint("GET", "/api/invites", InvitesResponse),
  createInvite: endpoint("POST", "/api/invites", CreateInviteResponse),
  revokeInvite: endpoint("DELETE", "/api/invites/:id", Ok),

  desktops: endpoint("GET", "/api/desktops", DesktopsResponse),
  pairInfo: endpoint("GET", "/api/pair/:userCode", PairInfoResponse),
  pairApprove: endpoint("POST", "/api/pair/:userCode/approve", PairApproveResponse, PairApproveBody),
  pairDeny: endpoint("POST", "/api/pair/:userCode/deny", Ok),

  putPushPrefs: endpoint("PUT", "/api/push/prefs/:desktopId", PushPrefsResponse, PushPrefsBody),
} as const;
