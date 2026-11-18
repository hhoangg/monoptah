// Chromium DevTools virtual WebAuthn authenticator (the `WebAuthn` CDP domain). Each browser context gets its own
// authenticator; a credential is moved between contexts with getCredentials / addCredential.
import type { BrowserContext, CDPSession, Page } from "@playwright/test";

/** The shape `WebAuthn.getCredentials` returns and `WebAuthn.addCredential` takes. */
export interface VirtualCredential {
  credentialId: string;
  isResidentCredential: boolean;
  rpId?: string;
  privateKey: string;
  userHandle?: string;
  signCount: number;
  largeBlob?: string;
}

export class VirtualAuthenticatorHandle {
  private readonly cdp: CDPSession;
  readonly authenticatorId: string;

  constructor(cdp: CDPSession, authenticatorId: string) {
    this.cdp = cdp;
    this.authenticatorId = authenticatorId;
  }

  async credentials(): Promise<VirtualCredential[]> {
    const { credentials } = await this.cdp.send("WebAuthn.getCredentials", { authenticatorId: this.authenticatorId });
    return credentials;
  }

  async addCredential(credential: VirtualCredential): Promise<void> {
    await this.cdp.send("WebAuthn.addCredential", { authenticatorId: this.authenticatorId, credential });
  }

  async removeCredential(credentialId: string): Promise<void> {
    await this.cdp.send("WebAuthn.removeCredential", { authenticatorId: this.authenticatorId, credentialId });
  }

  async dispose(): Promise<void> {
    await this.cdp.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId: this.authenticatorId }).catch(() => undefined);
    await this.cdp.send("WebAuthn.disable").catch(() => undefined);
  }
}

/** Adds a CTAP2 internal authenticator with resident keys, user verification and automatic presence. */
export async function addVirtualAuthenticator(context: BrowserContext, page: Page): Promise<VirtualAuthenticatorHandle> {
  const cdp = await context.newCDPSession(page);
  await cdp.send("WebAuthn.enable", { enableUI: false });
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      ctap2Version: "ctap2_1",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  return new VirtualAuthenticatorHandle(cdp, authenticatorId);
}
