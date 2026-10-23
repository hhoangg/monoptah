import { invoke } from "@tauri-apps/api/core";
import type { UserQuestionReply } from "../../sessions/model/userQuestion";

export type TelegramStatus = {
  configured: boolean;
  enabled: boolean;
  botUsername: string;
  chatId?: number;
  chatTitle?: string;
  isForum?: boolean;
  canManageTopics?: boolean;
  problem?: string;
};

export type ConnectLink = { code: string; url: string; fallback: string };

export type LinkCode = { code: string; command: string };

export type ForwardPayload = {
  project: string;
  sessionId: string;
  requestId: number;
  topicTitle: string;
  header: string;
  context: string;
  questions: {
    id: string;
    header?: string;
    prompt: string;
    multiSelect: boolean;
    allowCustom: boolean;
    options: { id: string; label: string; description?: string }[];
  }[];
};

export const TELEGRAM_ANSWER_EVENT = "monocode-telegram-answer";

export type TelegramAnswerEvent = {
  key: string;
  sessionId: string;
  requestId: number;
  reply: UserQuestionReply;
  by: string;
};

export type TelegramQuestionStatus =
  "Answered on desktop" | "Skipped" | "Cancelled";

export function telegramStatus(project: string): Promise<TelegramStatus> {
  return invoke<TelegramStatus>("telegram_status", { project });
}

export function telegramSetToken(
  project: string,
  token: string,
): Promise<TelegramStatus> {
  return invoke<TelegramStatus>("telegram_set_token", { project, token });
}

export function telegramConnectLink(project: string): Promise<ConnectLink> {
  return invoke<ConnectLink>("telegram_connect_link", { project });
}

export function telegramSetEnabled(
  project: string,
  enabled: boolean,
): Promise<TelegramStatus> {
  return invoke<TelegramStatus>("telegram_set_enabled", { project, enabled });
}

export function telegramDisconnect(project: string): Promise<void> {
  return invoke<void>("telegram_disconnect", { project });
}

export function telegramRemoveWebhook(project: string): Promise<void> {
  return invoke<void>("telegram_remove_webhook", { project });
}

export function telegramLinkCode(
  project: string,
  sessionId: string,
): Promise<LinkCode> {
  return invoke<LinkCode>("telegram_link_code", { project, sessionId });
}

export function telegramSessionTopic(
  project: string,
  sessionId: string,
): Promise<number | null> {
  return invoke<number | null>("telegram_session_topic", {
    project,
    sessionId,
  });
}

export function telegramForwardQuestion(
  payload: ForwardPayload,
): Promise<void> {
  return invoke<void>("telegram_forward_question", { payload });
}

export function telegramQuestionClosed(
  sessionId: string,
  requestId: number,
  status: TelegramQuestionStatus,
): Promise<void> {
  return invoke<void>("telegram_question_closed", {
    sessionId,
    requestId,
    status,
  });
}

export function telegramAnswerResult(key: string, ok: boolean): Promise<void> {
  return invoke<void>("telegram_answer_result", { key, ok });
}
