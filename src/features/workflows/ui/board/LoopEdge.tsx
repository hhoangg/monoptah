import {
  BaseEdge,
  EdgeLabelRenderer,
  getSmoothStepPath,
  type EdgeProps,
} from "@xyflow/react";
import type { LoopSpec } from "../../model/workflow";
import type { FlowEdge } from "./boardModel";

const CONDITION_MAX = 28;

/** "Mono decides · max 3", or "If <condition> · max 3". */
export function loopLabel(loop: LoopSpec, full = false): string {
  const condition = loop.decide.trim();
  const shown = condition.length > CONDITION_MAX && !full ? `${condition.slice(0, CONDITION_MAX - 1)}…` : condition;
  return `${loop.decide === "mono" ? "Mono decides" : `If ${shown}`} · max ${loop.maxRounds}`;
}

/**
 * The way back of a loop: out of the right side of the later node, around, and into the right
 * side of the earlier one, dashed in the Mono's color with a small label on its upright run.
 */
export function LoopEdge({
  id,
  sourceX,
  sourceY,
  targetX,
  targetY,
  sourcePosition,
  targetPosition,
  markerEnd,
  selected,
  data,
}: EdgeProps<FlowEdge>) {
  const [path, labelX, labelY] = getSmoothStepPath({
    sourceX,
    sourceY,
    targetX,
    targetY,
    sourcePosition,
    targetPosition,
    offset: 26,
    borderRadius: 12,
  });
  return (
    <>
      <BaseEdge id={id} path={path} markerEnd={markerEnd} className="loop-edge-path" interactionWidth={16} />
      {data?.loop ? (
        <EdgeLabelRenderer>
          <span
            data-loop-label
            data-selected={selected || undefined}
            title={loopLabel(data.loop, true)}
            className="loop-edge-label nodrag nopan"
            style={{
              transform: `translate(-50%, -50%) translate(${labelX + 12}px, ${labelY}px) rotate(90deg)`,
            }}
          >
            {loopLabel(data.loop)}
          </span>
        </EdgeLabelRenderer>
      ) : null}
    </>
  );
}

export const edgeTypes = { loop: LoopEdge };
