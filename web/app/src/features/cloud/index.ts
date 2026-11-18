// Public API of the cloud feature for the other WS-C tasks (the approval card composition, the start
// picker and the move actions). Registrations are in `register.ts`, imported by main.tsx; this file
// has no side effects.
export { CloudComposerTop, type CloudComposerTopProps } from "./session/CloudComposerTop";
export { CloudChip } from "./session/CloudChip";
export { StepList, currentStepText, moveItems, startupItems, startupItemsFromCard, type StepItem } from "./StepList";
export { COST_NOTICES, KEPT_NOTE, NOT_CONNECTED_NOTE, READ_ONLY_NOTE, STARTING_REFUSAL, STOPPED_NOTE, UNAVAILABLE_REFUSAL, STOP_CONFIRM, WAKING_NOTE } from "./copy";
export { KIND_LABELS, MOVE_STEP_LABELS, STARTUP_STEP_LABELS, STATE_LABELS, STEP_STATUS_LABELS, isBillableKind, isNonLocalKind, kindLabel, placementOf, stateLabel, useCanControl, type Placement } from "./model";
