// In-memory token bucket and breach counter for the per-socket relay rate limits (architecture.md 4.4).
// Rebuilt from scratch after hibernation: best effort by design.

export class TokenBucket {
  #tokens: number;
  #last: number;
  readonly #capacity: number;
  readonly #perMs: number;

  constructor(capacity: number, perSecond: number, now: number) {
    this.#capacity = capacity;
    this.#perMs = perSecond / 1000;
    this.#tokens = capacity;
    this.#last = now;
  }

  /** Takes `cost` tokens when available. */
  take(cost: number, now: number): boolean {
    this.#tokens = Math.min(this.#capacity, this.#tokens + Math.max(0, now - this.#last) * this.#perMs);
    this.#last = now;
    if (this.#tokens < cost) return false;
    this.#tokens -= cost;
    return true;
  }
}

export interface RateConfig {
  messagesPerSecond: number;
  burst: number;
  bytesPerSecond: number;
}

/** Message and byte buckets of one socket plus the times of its recent breaches. */
export class SocketLimiter {
  readonly #messages: TokenBucket;
  readonly #bytes: TokenBucket;
  #breaches: number[] = [];

  constructor(config: RateConfig, now: number) {
    this.#messages = new TokenBucket(config.burst, config.messagesPerSecond, now);
    this.#bytes = new TokenBucket(config.bytesPerSecond, config.bytesPerSecond, now);
  }

  /** True when the frame may pass. */
  allow(bytes: number, now: number): boolean {
    const okMessage = this.#messages.take(1, now);
    const okBytes = this.#bytes.take(bytes, now);
    return okMessage && okBytes;
  }

  /** Records a breach; returns true when `limit` breaches fall within `windowMs`. */
  breach(now: number, limit: number, windowMs: number): boolean {
    this.#breaches = this.#breaches.filter((t) => now - t < windowMs);
    this.#breaches.push(now);
    return this.#breaches.length >= limit;
  }
}
