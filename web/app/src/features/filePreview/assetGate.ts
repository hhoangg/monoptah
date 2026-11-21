// At most N jobs at a time, the rest in order. Used so the images of a preview ask the desktop a few at a time
// (the desktop answers a device with at most 3 `file.prepare` in flight and refuses the next).

export class Gate {
  #free: number;
  readonly #waiting: Array<() => void> = [];
  constructor(size: number) {
    this.#free = size;
  }

  async run<T>(job: () => Promise<T>, signal?: AbortSignal): Promise<T> {
    signal?.throwIfAborted();
    if (this.#free > 0) this.#free--;
    else {
      await new Promise<void>((resolve, reject) => {
        const turn = () => {
          signal?.removeEventListener("abort", leave);
          resolve();
        };
        const leave = () => {
          const at = this.#waiting.indexOf(turn);
          if (at >= 0) this.#waiting.splice(at, 1);
          reject(signal?.reason);
        };
        this.#waiting.push(turn);
        signal?.addEventListener("abort", leave, { once: true });
      });
    }
    try {
      return await job();
    } finally {
      const next = this.#waiting.shift();
      if (next) next();
      else this.#free++;
    }
  }
}

/** The desktop's allowance of `file.prepare` for one device. */
export const PREPARE_CONCURRENCY = 3;

const gates = new Map<string, Gate>();

/** The one gate of a desktop: every preview of that desktop in this tab shares it, so two previews cannot exceed the allowance together. */
export function prepareGate(desktopId: string): Gate {
  let gate = gates.get(desktopId);
  if (!gate) {
    gate = new Gate(PREPARE_CONCURRENCY);
    gates.set(desktopId, gate);
  }
  return gate;
}
