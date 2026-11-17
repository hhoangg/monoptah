/** Outer frame limits of architecture.md 4.4: 20 byte header minimum, 20 + 65,535 maximum. */
export const MIN_FRAME = 20;
export const MAX_FRAME = 65_555;
/** Prototype header: byte 0 version, byte 1 type, bytes 2..9 ASCII device id padded with 0. */
export const DEVICE_ID_OFFSET = 2;
export const DEVICE_ID_LEN = 8;
export const TYPE_TRANSPORT = 0x03;
/** Spike only: asks the room for its counters, answered with a JSON text-free binary frame. */
export const TYPE_STATS = 0x7f;

export type FrameCheck = "ok" | "too_small" | "too_big";

export function checkFrameSize(length: number): FrameCheck {
  if (length < MIN_FRAME) return "too_small";
  if (length > MAX_FRAME) return "too_big";
  return "ok";
}

export function readDeviceId(frame: Uint8Array): string {
  let end = DEVICE_ID_OFFSET;
  while (end < DEVICE_ID_OFFSET + DEVICE_ID_LEN && frame[end] !== 0) end++;
  return new TextDecoder().decode(frame.subarray(DEVICE_ID_OFFSET, end));
}

export function writeDeviceId(frame: Uint8Array, id: string): void {
  const bytes = new TextEncoder().encode(id).subarray(0, DEVICE_ID_LEN);
  frame.fill(0, DEVICE_ID_OFFSET, DEVICE_ID_OFFSET + DEVICE_ID_LEN);
  frame.set(bytes, DEVICE_ID_OFFSET);
}

/** Builds a frame of exactly `length` bytes with a deterministic body, for tests and replay. */
export function buildFrame(length: number, deviceId: string, type = TYPE_TRANSPORT): Uint8Array<ArrayBuffer> {
  const frame = new Uint8Array(new ArrayBuffer(length));
  frame[0] = 1;
  frame[1] = type;
  writeDeviceId(frame, deviceId);
  for (let i = 10; i < length; i++) frame[i] = (i * 31 + 7) & 0xff;
  return frame;
}
