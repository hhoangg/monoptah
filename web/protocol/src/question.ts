// Stub owned later by WS-T, which extends it under W-17/W-18. It only re-exports the
// question wire types from methods.ts (defined in wire.ts) so imports stay stable.
export { QuestionOptionSchema, QuestionPromptSchema, QuestionReplySchema, QuestionSchema } from "./methods.ts";
export type { QuestionPrompt, QuestionReply } from "./methods.ts";
