/** Serializable state of a token bucket: what goes into the hibernation attachment. */
export interface BucketState {
  tokens: number;
  /** Epoch ms of the last refill. */
  at: number;
}

/**
 * Token bucket with an explicit clock. The state is a plain object so it can live in
 * `serializeAttachment` and be rebuilt after the Durable Object was evicted.
 */
export class TokenBucket {
  readonly capacity: number;
  readonly refillPerSec: number;
  private tokens: number;
  private at: number;

  constructor(capacity: number, refillPerSec: number, now: number, state?: BucketState) {
    this.capacity = capacity;
    this.refillPerSec = refillPerSec;
    if (state) {
      this.tokens = Math.min(capacity, Math.max(0, state.tokens));
      this.at = state.at;
      this.refill(now);
    } else {
      this.tokens = capacity;
      this.at = now;
    }
  }

  private refill(now: number): void {
    if (now > this.at) {
      this.tokens = Math.min(this.capacity, this.tokens + ((now - this.at) / 1000) * this.refillPerSec);
      this.at = now;
    }
  }

  /** Takes `cost` tokens if available. */
  take(cost: number, now: number): boolean {
    this.refill(now);
    if (this.tokens >= cost) {
      this.tokens -= cost;
      return true;
    }
    return false;
  }

  snapshot(): BucketState {
    return { tokens: this.tokens, at: this.at };
  }
}
