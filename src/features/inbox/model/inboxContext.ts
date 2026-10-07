import type { InboxItem } from "./githubTasks";
import { clickupIssueDetails, peekClickUpIssueDetails } from "./clickup";
import { jiraIssueDetails, peekJiraIssueDetails } from "./jira";
import { linearIssueDetails, peekLinearIssueDetails } from "./linear";

/** Load tracker context before opening a session, including from list actions. */
export async function inboxTrackerDescription(
  item: InboxItem,
  body?: string,
): Promise<string | undefined> {
  if (
    item.provider !== "linear" &&
    item.provider !== "jira" &&
    item.provider !== "clickup"
  ) {
    return body;
  }
  if (body !== undefined) return body;
  if (item.provider === "jira") {
    const key = item.identifier?.trim();
    if (!key) throw new Error("Missing Jira issue key");
    return (peekJiraIssueDetails(key) ?? (await jiraIssueDetails(key))).body;
  }
  if (item.provider === "clickup") {
    // `identifier` is display text (a custom id or `#<id>`); the API only
    // accepts the raw task id.
    const taskId = item.id?.trim();
    if (!taskId) throw new Error("Missing ClickUp task");
    return (
      peekClickUpIssueDetails(taskId) ?? (await clickupIssueDetails(taskId))
    ).body;
  }
  if (!item.id) throw new Error("Missing Linear issue");
  return (
    peekLinearIssueDetails(item.id) ?? (await linearIssueDetails(item.id))
  ).body;
}
