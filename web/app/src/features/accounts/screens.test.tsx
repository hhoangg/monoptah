import { QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const lib = vi.hoisted(() => ({
  startRegistration: vi.fn(),
  startAuthentication: vi.fn(),
  browserSupportsWebAuthn: vi.fn(() => true),
}));
vi.mock("@simplewebauthn/browser", () => lib);

import { createQueryClient } from "@/lib/queryClient";
import { createAppRouter } from "@/router";
import { setApiTransport, type ApiTransport } from "./api";
import { createMockServer } from "./mock/transport";
import { createMockState, newRecoveryBatch, SEED_INVITE_CODE, type SeedOptions } from "./mock/state";
import { normalizeRecoveryCode, recoveryCodesFileName, recoveryCodesText } from "./recoveryCodesFile";
import { RecoveryCodes } from "./RecoveryCodes";
import { readInviteFromFragment, scrubInviteFragment } from "./RegisterScreen";

const CREDENTIAL = { id: "cred", rawId: "cred", type: "public-key", response: {}, clientExtensionResults: {} };

let restore: (() => void) | undefined;
let sent: Array<{ url: string; body: unknown }> = [];

function install(options: SeedOptions) {
  const state = createMockState(options);
  const server = createMockServer({ state });
  const transport: ApiTransport = (url, init) => {
    if (init.body) sent.push({ url, body: JSON.parse(String(init.body)) });
    return server.transport(url, init);
  };
  restore = setApiTransport(transport);
  return state;
}

function mount(url: string) {
  const queryClient = createQueryClient();
  const router = createAppRouter({ history: createMemoryHistory({ initialEntries: [url] }), queryClient });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { queryClient, router };
}

beforeEach(() => {
  sent = [];
  window.localStorage.clear();
  window.sessionStorage.clear();
  lib.startRegistration.mockResolvedValue(CREDENTIAL);
  lib.startAuthentication.mockResolvedValue(CREDENTIAL);
  lib.browserSupportsWebAuthn.mockReturnValue(true);
});
afterEach(() => {
  restore?.();
  vi.clearAllMocks();
  window.history.replaceState(null, "", "/");
});

function storedText(): string {
  return JSON.stringify([...Object.entries(localStorage), ...Object.entries(sessionStorage)]);
}

describe("invite fragment", () => {
  it("is read without a side effect, so a retried render finds it again, and scrubbed separately", () => {
    window.history.replaceState({ key: "k1" }, "", "/register#invite=ABC");
    expect(readInviteFromFragment()).toBe("ABC");
    expect(readInviteFromFragment()).toBe("ABC");
    expect(window.location.hash).toBe("#invite=ABC");
    scrubInviteFragment();
    expect(window.location.hash).toBe("");
    expect(window.history.state).toEqual({ key: "k1" });
  });
});

describe("register", () => {
  it("runs the happy path: invite from the fragment, codes, gate, second passkey prompt", async () => {
    install({ signedIn: false });
    window.history.replaceState(null, "", `/register#invite=${SEED_INVITE_CODE}`);
    const user = userEvent.setup();
    const { queryClient } = mount("/register");

    await screen.findByRole("heading", { level: 1, name: "Create your account" });
    expect(screen.getByLabelText("Invite code")).toHaveValue(SEED_INVITE_CODE);
    // The fragment is gone from the address bar once read.
    expect(window.location.hash).toBe("");
    expect(screen.getByText("Your handle cannot be changed later.")).toBeInTheDocument();

    await user.type(screen.getByLabelText("Handle"), "ada");
    await user.type(screen.getByLabelText("Display name"), "Ada Lovelace");
    await user.click(screen.getByRole("button", { name: /Create passkey/ }));

    await screen.findByRole("heading", { level: 1, name: "Save your recovery codes" });
    const list = screen.getByRole("list", { name: "Recovery codes" });
    const codes = within(list).getAllByRole("listitem");
    expect(codes).toHaveLength(10);
    const codeTexts = codes.map((item) => item.textContent?.replace(/^\d+/, "") ?? "");

    // Continue stays disabled until the box is ticked.
    const next = screen.getByRole("button", { name: "Continue" });
    expect(next).toBeDisabled();
    await user.click(screen.getByLabelText("I saved these codes"));
    expect(next).toBeEnabled();

    // Never in web storage, the query cache or the mutation cache.
    const cached = JSON.stringify([queryClient.getQueryCache().getAll().map((q) => q.state.data), queryClient.getMutationCache().getAll().map((m) => m.state.data)]);
    for (const code of codeTexts) {
      expect(storedText()).not.toContain(code);
      expect(cached).not.toContain(code);
    }

    await user.click(next);
    await screen.findByRole("heading", { level: 1, name: "Add a second passkey" });
    expect(screen.getByRole("button", { name: "Skip for now" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Skip for now" }));
    await waitFor(() => expect(screen.queryByRole("heading", { level: 1, name: "Add a second passkey" })).toBeNull());
  });

  it("shows one generic message for a bad invite", async () => {
    install({ signedIn: false });
    const user = userEvent.setup();
    mount("/register");
    await screen.findByRole("heading", { level: 1, name: "Create your account" });
    await user.type(screen.getByLabelText("Invite code"), "WRONG-CODE");
    await user.type(screen.getByLabelText("Handle"), "ada");
    await user.type(screen.getByLabelText("Display name"), "Ada");
    await user.click(screen.getByRole("button", { name: /Create passkey/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This invite is not valid. Ask the person who invited you for a new link.");
    expect(lib.startRegistration).not.toHaveBeenCalled();
  });

  it("gives the same message when invites are paused", async () => {
    install({ signedIn: false, invitesPaused: true });
    const user = userEvent.setup();
    mount("/register");
    await screen.findByRole("heading", { level: 1, name: "Create your account" });
    await user.type(screen.getByLabelText("Invite code"), SEED_INVITE_CODE);
    await user.type(screen.getByLabelText("Handle"), "ada");
    await user.type(screen.getByLabelText("Display name"), "Ada");
    await user.click(screen.getByRole("button", { name: /Create passkey/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This invite is not valid.");
  });

  it("validates the handle with the contract regex before any request", async () => {
    install({ signedIn: false });
    const user = userEvent.setup();
    mount("/register");
    await screen.findByRole("heading", { level: 1, name: "Create your account" });
    await user.type(screen.getByLabelText("Invite code"), SEED_INVITE_CODE);
    await user.type(screen.getByLabelText("Handle"), "a!");
    await user.type(screen.getByLabelText("Display name"), "Ada");
    await user.click(screen.getByRole("button", { name: /Create passkey/ }));
    expect(await screen.findByText("Use 3 to 32 lowercase letters, digits, - or _.")).toBeInTheDocument();
    expect(sent).toHaveLength(0);
  });
});

describe("login", () => {
  it("does not promise cross-device sign-in", async () => {
    install({ signedIn: false });
    mount("/login");
    await screen.findByRole("heading", { level: 1, name: "Sign in to Monoptah" });
    expect(screen.getByText(/If your browser offers it/)).toBeInTheDocument();
    expect(screen.queryByText(/another device/)).toBeNull();
  });

  it("shows a soft notice, not an error, when the prompt is cancelled", async () => {
    install({ signedIn: false });
    lib.startAuthentication.mockRejectedValue(Object.assign(new Error("x"), { name: "NotAllowedError" }));
    const user = userEvent.setup();
    mount("/login");
    await screen.findByRole("heading", { level: 1, name: "Sign in to Monoptah" });
    await user.click(screen.getByRole("button", { name: /Sign in with passkey/ }));
    const notice = await screen.findByText(/Sign-in was cancelled/);
    expect(notice.closest("[data-variant]")).toHaveAttribute("data-variant", "info");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("links to recover and install and follows the redirect after sign-in", async () => {
    install({ signedIn: false });
    const user = userEvent.setup();
    const { router } = mount("/login?redirect=%2Fdesktops");
    await screen.findByRole("heading", { level: 1, name: "Sign in to Monoptah" });
    expect(screen.getByRole("link", { name: /Lost your passkey/ })).toHaveAttribute("href", "/recover");
    expect(screen.getByRole("link", { name: /Install the app/ })).toHaveAttribute("href", "/install");
    await user.click(screen.getByRole("button", { name: /Sign in with passkey/ }));
    await waitFor(() => expect(router.state.location.pathname).toBe("/desktops"));
  });
});

describe("recover", () => {
  it("normalizes the code, then forces a new passkey with no skip", async () => {
    const state = install({ signedIn: false });
    const code = newRecoveryBatch(state)[0]!;
    const messy = ` ${code.toLowerCase().replace(/(.{4})/g, "$1 - ")} `;
    const user = userEvent.setup();
    mount("/recover");
    await screen.findByLabelText("Handle");
    await user.type(screen.getByLabelText("Handle"), "dev-user");
    await user.type(screen.getByLabelText("Recovery code"), messy);
    await user.click(screen.getByRole("button", { name: "Recover account" }));

    await screen.findByRole("heading", { level: 1, name: "Add a new passkey" });
    expect(screen.queryByRole("button", { name: /skip/i })).toBeNull();
    const recoverCall = sent.find((entry) => entry.url.endsWith("/api/auth/recover"));
    expect(recoverCall?.body).toEqual({ handle: "dev-user", code: normalizeRecoveryCode(code) });
    expect(/^[0-9A-Z]+$/.test((recoverCall?.body as { code: string }).code)).toBe(true);
  });

  it("renders the add passkey step for a recoveryOnly session on load", async () => {
    install({ signedIn: true, recoveryOnly: true });
    mount("/recover");
    expect(await screen.findByRole("heading", { level: 1, name: "Add a new passkey" })).toBeInTheDocument();
  });

  it("sends the user on to the passkey step when the session is recovery-only and /api/me answers 403", async () => {
    const state = install({ signedIn: true, recoveryOnly: true });
    mount("/recover");
    await screen.findByRole("heading", { level: 1, name: "Add a new passkey" });
    expect(state.recoveryOnly).toBe(true);
  });

  it("answers every failure with the same generic message", async () => {
    install({ signedIn: false });
    const user = userEvent.setup();
    mount("/recover");
    await screen.findByLabelText("Handle");
    await user.type(screen.getByLabelText("Handle"), "nobody");
    await user.type(screen.getByLabelText("Recovery code"), "AAAA-BBBB");
    await user.click(screen.getByRole("button", { name: "Recover account" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("That handle or recovery code is not valid.");
  });

  it("offers new codes when 2 or fewer are left", async () => {
    const state = install({ signedIn: false });
    const batch = newRecoveryBatch(state);
    for (const spent of batch.slice(2)) state.recoveryCodes.delete(normalizeRecoveryCode(spent));
    const user = userEvent.setup();
    mount("/recover");
    await screen.findByLabelText("Handle");
    await user.type(screen.getByLabelText("Handle"), "dev-user");
    await user.type(screen.getByLabelText("Recovery code"), batch[0]!);
    await user.click(screen.getByRole("button", { name: "Recover account" }));
    await screen.findByRole("heading", { level: 1, name: "Add a new passkey" });
    await user.click(screen.getByRole("button", { name: "Add a passkey" }));
    await screen.findByRole("heading", { level: 1, name: "Get new recovery codes" });
    // Forced: no way to skip the regenerate step.
    expect(screen.queryByRole("button", { name: /not now|skip/i })).toBeNull();
    await user.click(screen.getByRole("button", { name: /Generate new codes/ }));
    await screen.findByRole("heading", { level: 1, name: "Save your new recovery codes" });
    expect(within(screen.getByRole("list", { name: "Recovery codes" })).getAllByRole("listitem")).toHaveLength(10);
  });
});

describe("forced regenerate", () => {
  it("replaces the settings stub while recoveryAckAt is null and keeps the codes until Continue", async () => {
    install({ signedIn: true, recoveryAckAt: null });
    const user = userEvent.setup();
    mount("/settings");
    await screen.findByRole("heading", { level: 1, name: "Get new recovery codes" });
    await user.click(screen.getByRole("button", { name: /Generate recovery codes/ }));
    await screen.findByRole("heading", { level: 1, name: "Save your new recovery codes" });
    expect(screen.getByRole("button", { name: "Continue" })).toBeDisabled();
    await user.click(screen.getByLabelText("I saved these codes"));
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByRole("heading", { level: 1, name: "Settings" });
  });
});

describe("RecoveryCodes", () => {
  const codes = Array.from({ length: 10 }, (_, i) => `CODE${i}-ABCDE`);

  it("downloads a text file named with the handle through a revoked object URL", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const create = vi.fn(() => "blob:test");
    const revoke = vi.fn();
    Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke });
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (this: HTMLAnchorElement) {
      expect(this.download).toBe("monoptah-recovery-codes-ada.txt");
    });
    const user = userEvent.setup();
    render(<RecoveryCodes handle="ada" codes={codes} onContinue={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Download" }));
    expect(click).toHaveBeenCalledOnce();
    expect(create).toHaveBeenCalledOnce();
    // Not revoked in the click task (WebKit may start the download later), but after a delay.
    expect(revoke).not.toHaveBeenCalled();
    vi.advanceTimersByTime(30_000);
    expect(revoke).toHaveBeenCalledWith("blob:test");
    click.mockRestore();
    vi.useRealTimers();
  });

  it("prints through window.print and keeps one marked print area", async () => {
    const print = vi.spyOn(window, "print").mockImplementation(() => undefined);
    const user = userEvent.setup();
    const { container } = render(<RecoveryCodes handle="ada" codes={codes} onContinue={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Print" }));
    expect(print).toHaveBeenCalledOnce();
    const area = container.querySelector("[data-print-area]");
    expect(area).not.toBeNull();
    expect(area?.querySelectorAll("li")).toHaveLength(10);
    print.mockRestore();
  });

  it("copies the handle and the codes", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    render(<RecoveryCodes handle="ada" codes={codes} onContinue={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(recoveryCodesText("ada", codes));
    expect(recoveryCodesText("ada", codes)).toContain("ada");
    await screen.findByText("Codes copied to the clipboard.");
  });
});

describe("recovery code helpers", () => {
  it("normalizes spaces, dashes, case and Crockford look-alikes", () => {
    expect(normalizeRecoveryCode(" ab12-cdoI l ")).toBe("AB12CD011");
  });
  it("builds a safe file name", () => {
    expect(recoveryCodesFileName("a/b..c")).toBe("monoptah-recovery-codes-abc.txt");
  });
});
