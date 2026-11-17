// Mutation adapter (architecture.md 3.2, F-09). A command id is kept per `(method, params)` signature
// until the call succeeds, so a user retry after a lost reply carries the same id and the desktop answers
// from its receipt table instead of running it twice (mobile/.../WorkspaceProvider.tsx:418-436).
// A mutation is NEVER re-sent by this layer: not after a reconnect, not by TanStack Query (`retry: 0`).
import { useMutation } from "@tanstack/react-query";
import type { MethodName, ParamsOf, ResultOf } from "@monoptah/protocol";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, randomBytes, utf8ToBytes } from "@noble/hashes/utils.js";
import { useLinkConnection, type LinkRequestError } from "@/link";
import type { LinkClient } from "./client.ts";

/** At most this many signatures keep an id after failures (mobile keeps 128). */
export const MAX_PENDING_COMMAND_IDS = 128;

/** JSON with sorted object keys, so equal params give the same signature whatever their key order. */
export function stableStringify(value: unknown): string {
  return JSON.stringify(value, (_key, inner: unknown) => {
    if (inner && typeof inner === "object" && !Array.isArray(inner)) {
      const record = inner as Record<string, unknown>;
      return Object.fromEntries(Object.keys(record).sort().map((key) => [key, record[key]]));
    }
    return inner;
  });
}

export function commandSignature(method: string, params: unknown): string {
  return stableStringify([method, params ?? {}]);
}

/**
 * The key of the id table: SHA-256 (hex) of the signature. A `conversation.send` signature holds its base64
 * attachments (up to 8 MiB each), so the table keeps 64 characters per entry, not the whole call.
 */
export function commandKey(method: string, params: unknown): string {
  return bytesToHex(sha256(utf8ToBytes(commandSignature(method, params))));
}

/** A random command id: 32 lowercase hex characters (the desktop accepts at most 128). */
export function makeCommandId(): string {
  return Array.from(randomBytes(16), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

export class Mutator {
  readonly #ids = new Map<string, string>();
  readonly #client: LinkClient;
  readonly #makeId: () => string;

  constructor(client: LinkClient, makeId: () => string = makeCommandId) {
    this.#client = client;
    this.#makeId = makeId;
  }

  /** The keys of the held ids (hashes, never the params). */
  pendingKeys(): string[] {
    return [...this.#ids.keys()];
  }

  /** The command id currently held for this call, if a failure left one. */
  pendingId(method: string, params: unknown): string | undefined {
    return this.#ids.get(commandKey(method, params));
  }

  /**
   * Sends the call with the id of its signature (a new one when none is held). The id is dropped on
   * success and kept on failure, for a retry that the user starts.
   */
  async mutate<M extends MethodName>(method: M, params: ParamsOf<M>): Promise<ResultOf<M>> {
    const signature = commandKey(method, params);
    let id = this.#ids.get(signature);
    if (!id) {
      id = this.#makeId();
      this.#ids.set(signature, id);
    }
    try {
      const result = await this.#client.request(method, params, { id });
      this.#ids.delete(signature);
      return result;
    } catch (reason) {
      if (this.#ids.size > MAX_PENDING_COMMAND_IDS) {
        const oldest = this.#ids.keys().next().value;
        if (oldest !== undefined) this.#ids.delete(oldest);
      }
      throw reason;
    }
  }
}

const mutators = new WeakMap<object, Mutator>();

/** One mutator per connection, so every caller of a desktop shares the ids. */
export function getMutator(client: LinkClient): Mutator {
  let mutator = mutators.get(client);
  if (!mutator) {
    mutator = new Mutator(client);
    mutators.set(client, mutator);
  }
  return mutator;
}

/** `useMutation` over the mutator of the nearest desktop. No automatic retry. */
export function useLinkMutation<M extends MethodName>(method: M) {
  const client = useLinkConnection();
  return useMutation<ResultOf<M>, LinkRequestError, ParamsOf<M>>({
    networkMode: "always",
    retry: 0,
    mutationFn: (params) => getMutator(client).mutate(method, params),
  });
}
