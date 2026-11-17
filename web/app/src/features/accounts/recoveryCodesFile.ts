// Plain helpers for the recovery codes screen. Nothing here touches web storage: the codes live in
// component state only (security.md 3, "shown once").

/** Crockford base32 input made canonical: no spaces or dashes, upper case, O is 0, I and L are 1. */
export function normalizeRecoveryCode(input: string): string {
  return input.replace(/[\s-]/g, "").toUpperCase().replace(/O/g, "0").replace(/[IL]/g, "1");
}

/** File name for the downloaded codes, for example `monoptah-recovery-codes-ada.txt`. */
export function recoveryCodesFileName(handle: string): string {
  const safe = handle.replace(/[^a-z0-9_-]/gi, "") || "account";
  return `monoptah-recovery-codes-${safe}.txt`;
}

/** Text of the download and of Copy: the handle, a short warning, then one code per line. */
export function recoveryCodesText(handle: string, codes: readonly string[]): string {
  return [
    "Monoptah recovery codes",
    `Account: ${handle}`,
    "",
    "Each code works once. Keep this file somewhere safe and private.",
    "",
    ...codes,
    "",
  ].join("\n");
}

const REVOKE_DELAY_MS = 30_000;

/** Saves text as a file through a Blob and an object URL that is revoked a little after the click. */
export function downloadTextFile(fileName: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain;charset=utf-8" }));
  try {
    const link = document.createElement("a");
    link.href = url;
    link.download = fileName;
    link.rel = "noopener";
    document.body.append(link);
    link.click();
    link.remove();
  } finally {
    // Some WebKit builds start the download after the click task; revoking at once can fail it.
    setTimeout(() => URL.revokeObjectURL(url), REVOKE_DELAY_MS);
  }
}
