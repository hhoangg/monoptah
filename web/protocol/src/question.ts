// Copied from src/features/sessions/model/userQuestion.ts at ca73ac2b
// Question card types (W-17). The wire shapes are defined once in wire.ts and re-exported by
// methods.ts; this file re-exports them and adds the element types the card uses.
// Types only.
import type { QuestionPrompt, QuestionReply } from "./methods.ts";

export { QuestionOptionSchema, QuestionPromptSchema, QuestionReplySchema, QuestionSchema } from "./methods.ts";
export type { QuestionPrompt, QuestionReply } from "./methods.ts";

/** One question of a prompt (desktop `UserQuestion`). */
export type Question = QuestionPrompt["questions"][number];
/** One selectable option of a question (desktop `UserQuestionOption`). */
export type QuestionOption = Question["options"][number];
/** The `answered` arm of a reply. */
export type AnsweredQuestionReply = Extract<QuestionReply, { kind: "answered" }>;
