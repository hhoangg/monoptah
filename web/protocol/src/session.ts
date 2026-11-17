// Stub owned later by WS-T, which extends it under W-17/W-18. It only re-exports the
// session card and Attachment wire types from methods.ts (Attachment is defined in wire.ts).
export { AttachmentSchema, AttachmentsSchema, SessionCardSchema } from "./methods.ts";
export type { Attachment, SessionCard } from "./methods.ts";
