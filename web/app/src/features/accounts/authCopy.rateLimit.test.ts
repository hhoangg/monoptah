import { describe, expect, it } from "vitest";
import { desktopErrorMessage } from "@/features/desktops/parts";
import { settingsErrorMessage } from "@/features/settings/copy";
import { ApiError } from "./api";
import { passkeyErrorMessage, rateLimitedMessage, signInErrorMessage } from "./authCopy";

function limited(retryAfter: number | null) {
  return new ApiError({ status: 429, code: "rate_limited", message: "Too many requests", retryAfter });
}

describe("rateLimitedMessage", () => {
  it("names the wait when it is known", () => {
    expect(rateLimitedMessage(1)).toBe("Too many requests. Try again in 1 second.");
    expect(rateLimitedMessage(12)).toBe("Too many requests. Try again in 12 seconds.");
    expect(rateLimitedMessage(60)).toBe("Too many requests. Try again in 1 minute.");
    expect(rateLimitedMessage(61)).toBe("Too many requests. Try again in 2 minutes.");
  });

  it("asks for a moment without a Retry-After", () => {
    expect(rateLimitedMessage(null)).toBe("Too many requests. Wait a moment and try again.");
  });

  it("is the one text every error helper uses for rate_limited", () => {
    const error = limited(9);
    const text = rateLimitedMessage(9);
    expect(settingsErrorMessage(error)).toBe(text);
    expect(desktopErrorMessage(error)).toBe(text);
    expect(signInErrorMessage(error)).toBe(text);
    expect(passkeyErrorMessage(error)).toBe(text);
  });
});
