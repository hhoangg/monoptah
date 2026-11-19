// Shared S-05 scenario: register and authenticate with a software authenticator through @simplewebauthn/server.
// Used by the Node test and by the workerd test, so both runtimes run the same code.
import {
  generateAuthenticationOptions,
  generateRegistrationOptions,
  verifyAuthenticationResponse,
  verifyRegistrationResponse,
} from "@simplewebauthn/server";
import { SpikeAuthenticator, type Algorithm } from "./authenticator.ts";

export const RP_ID = "localhost";
export const RP_ORIGIN = "http://localhost:8787";
/** COSE algorithm ids the server accepts: ES256, EdDSA, RS256 (the library default is [-8, -7, -257]). */
export const SUPPORTED_ALGORITHM_IDS = [-7, -8, -257];

export interface RoundTrip {
  algorithm: Algorithm;
  registered: boolean;
  verified: boolean;
  newCounter: number;
  /** Error message of a replayed assertion (same counter), or null if it was accepted. */
  replayError: string | null;
  /** Error message of an assertion with a lower counter, or null if it was accepted. */
  lowerCounterError: string | null;
  /** Counter 0 on both sides is accepted by design (authenticators without a counter). */
  zeroCounterAccepted: boolean;
  registerMs: number;
  verifyMs: number;
}

async function authenticate(
  device: SpikeAuthenticator,
  credential: { id: string; publicKey: Uint8Array<ArrayBuffer>; counter: number },
  counter?: number,
): Promise<{ verified: boolean; newCounter: number }> {
  const options = await generateAuthenticationOptions({ rpID: RP_ID, userVerification: "required" });
  const response = await device.get(options, { origin: RP_ORIGIN, rpId: RP_ID, ...(counter === undefined ? {} : { counter }) });
  const result = await verifyAuthenticationResponse({
    response,
    expectedChallenge: options.challenge,
    expectedOrigin: RP_ORIGIN,
    expectedRPID: RP_ID,
    requireUserVerification: true,
    credential,
  });
  return { verified: result.verified, newCounter: result.authenticationInfo.newCounter };
}

async function failure(run: () => Promise<unknown>): Promise<string | null> {
  try {
    await run();
    return null;
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

export async function roundTrip(algorithm: Algorithm, now: () => number): Promise<RoundTrip> {
  const device = await SpikeAuthenticator.generate(algorithm);
  const regOptions = await generateRegistrationOptions({
    rpName: "Monoptah spike",
    rpID: RP_ID,
    userName: "spike-user",
    attestationType: "none",
    supportedAlgorithmIDs: SUPPORTED_ALGORITHM_IDS,
    authenticatorSelection: { residentKey: "required", userVerification: "required" },
  });
  const created = await device.create(regOptions, { origin: RP_ORIGIN, rpId: RP_ID });
  const t0 = now();
  const registration = await verifyRegistrationResponse({
    response: created,
    expectedChallenge: regOptions.challenge,
    expectedOrigin: RP_ORIGIN,
    expectedRPID: RP_ID,
    requireUserVerification: true,
    supportedAlgorithmIDs: SUPPORTED_ALGORITHM_IDS,
  });
  const registerMs = now() - t0;
  const info = registration.registrationInfo;
  if (!registration.verified || !info) throw new Error("registration did not verify");
  const credential = { id: info.credential.id, publicKey: info.credential.publicKey, counter: info.credential.counter };

  const t1 = now();
  const first = await authenticate(device, credential);
  const verifyMs = now() - t1;

  const stored = { ...credential, counter: first.newCounter };
  const replayError = await failure(() => authenticate(device, stored, first.newCounter));
  const lowerCounterError = await failure(() => authenticate(device, stored, first.newCounter - 1 > 0 ? first.newCounter - 1 : 0));
  const zero = await authenticate(device, { ...credential, counter: 0 }, 0).then(
    (r) => r.verified,
    () => false,
  );

  return {
    algorithm,
    registered: registration.verified,
    verified: first.verified,
    newCounter: first.newCounter,
    replayError,
    lowerCounterError,
    zeroCounterAccepted: zero,
    registerMs,
    verifyMs,
  };
}

/** Mean and p95 (ms) of `n` full authentication verifications (options + signature + verify), after 5 warm-up runs. */
export async function timeVerify(algorithm: Algorithm, n: number, now: () => number): Promise<{ meanMs: number; p95Ms: number; maxMs: number }> {
  const device = await SpikeAuthenticator.generate(algorithm);
  const regOptions = await generateRegistrationOptions({ rpName: "Monoptah spike", rpID: RP_ID, userName: "timing", supportedAlgorithmIDs: SUPPORTED_ALGORITHM_IDS });
  const created = await device.create(regOptions, { origin: RP_ORIGIN, rpId: RP_ID });
  const reg = await verifyRegistrationResponse({
    response: created,
    expectedChallenge: regOptions.challenge,
    expectedOrigin: RP_ORIGIN,
    expectedRPID: RP_ID,
    supportedAlgorithmIDs: SUPPORTED_ALGORITHM_IDS,
  });
  const info = reg.registrationInfo;
  if (!info) throw new Error("no registration info");
  // Pre-build the assertions so only verifyAuthenticationResponse is timed.
  const prepared: { response: Awaited<ReturnType<SpikeAuthenticator["get"]>>; challenge: string }[] = [];
  for (let i = 0; i < n + 5; i += 1) {
    const options = await generateAuthenticationOptions({ rpID: RP_ID });
    prepared.push({ response: await device.get(options, { origin: RP_ORIGIN, rpId: RP_ID }), challenge: options.challenge });
  }
  const samples: number[] = [];
  for (const [i, item] of prepared.entries()) {
    const t = now();
    await verifyAuthenticationResponse({
      response: item.response,
      expectedChallenge: item.challenge,
      expectedOrigin: RP_ORIGIN,
      expectedRPID: RP_ID,
      credential: { id: info.credential.id, publicKey: info.credential.publicKey, counter: 0 },
    });
    if (i >= 5) samples.push(now() - t);
  }
  samples.sort((a, b) => a - b);
  const mean = samples.reduce((s, v) => s + v, 0) / samples.length;
  return { meanMs: mean, p95Ms: samples[Math.floor(samples.length * 0.95)] ?? 0, maxMs: samples[samples.length - 1] ?? 0 };
}

/** Pre-built registration and assertion responses of one software authenticator, so a request can verify them without
 *  paying for key generation or signing (used by the spike Worker to measure the CPU time of verification alone). */
export interface Fixture {
  registration: Awaited<ReturnType<SpikeAuthenticator["create"]>>;
  registrationChallenge: string;
  assertion: Awaited<ReturnType<SpikeAuthenticator["get"]>>;
  assertionChallenge: string;
  credential: { id: string; publicKey: Uint8Array<ArrayBuffer>; counter: number };
}

export async function buildFixture(algorithm: Algorithm): Promise<Fixture> {
  const device = await SpikeAuthenticator.generate(algorithm);
  const options = await generateRegistrationOptions({ rpName: "Monoptah spike", rpID: RP_ID, userName: "fixture", supportedAlgorithmIDs: SUPPORTED_ALGORITHM_IDS });
  const registration = await device.create(options, { origin: RP_ORIGIN, rpId: RP_ID });
  const verified = await verifyRegistrationResponse({
    response: registration,
    expectedChallenge: options.challenge,
    expectedOrigin: RP_ORIGIN,
    expectedRPID: RP_ID,
    supportedAlgorithmIDs: SUPPORTED_ALGORITHM_IDS,
  });
  const info = verified.registrationInfo;
  if (!info) throw new Error("no registration info");
  const authOptions = await generateAuthenticationOptions({ rpID: RP_ID });
  const assertion = await device.get(authOptions, { origin: RP_ORIGIN, rpId: RP_ID });
  return {
    registration,
    registrationChallenge: options.challenge,
    assertion,
    assertionChallenge: authOptions.challenge,
    credential: { id: info.credential.id, publicKey: info.credential.publicKey, counter: 0 },
  };
}

/** Verifies the fixture registration (`reg`) or assertion (`auth`): only library work, no key generation, no signing. */
export async function verifyFixture(fixture: Fixture, operation: "reg" | "auth"): Promise<boolean> {
  if (operation === "reg") {
    const result = await verifyRegistrationResponse({
      response: fixture.registration,
      expectedChallenge: fixture.registrationChallenge,
      expectedOrigin: RP_ORIGIN,
      expectedRPID: RP_ID,
      supportedAlgorithmIDs: SUPPORTED_ALGORITHM_IDS,
    });
    return result.verified;
  }
  const result = await verifyAuthenticationResponse({
    response: fixture.assertion,
    expectedChallenge: fixture.assertionChallenge,
    expectedOrigin: RP_ORIGIN,
    expectedRPID: RP_ID,
    credential: fixture.credential,
  });
  return result.verified;
}
