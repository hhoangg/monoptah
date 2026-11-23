// The "Pair a desktop" pieces (F-02), shared by /settings/desktops (DesktopsScreen) and the /pair page: the numbered steps
// and the paste field that continues to /pair/$code. Owner: WS-M.
import { useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { parsePairingInput } from "./pairingLink";
import { SettingsLayout } from "@/features/settings/SettingsLayout";
import { Button } from "@/ui/button";
import { Field } from "@/ui/Field";
import { PageHeader } from "@/ui/PageHeader";
import { SETTINGS_ANCHOR, SettingsCard } from "@/ui/SettingsCard";

export const PAIR_STEPS = [
  "On the desktop, open Settings > Web app > Pair. It shows a link and a QR code.",
  "Scan the QR code with this device, or paste the link or its 8 character code below.",
  "Confirm the match on the desktop. It then shows up here.",
] as const;

/** The numbered pairing steps. */
export function PairStepsList() {
  return (
    <ol className="m-0 flex max-w-prose list-decimal flex-col gap-1.5 pl-5 text-left text-small text-content">
      {PAIR_STEPS.map((step) => (
        <li key={step}>{step}</li>
      ))}
    </ol>
  );
}

export function PairSection({ empty }: { empty: boolean }) {
  const navigate = useNavigate();
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const target = parsePairingInput(value);
    if (!target) {
      setError("That is not a pairing link or code of this app. Paste the link shown on the desktop, or its 8 character code.");
      return;
    }
    setError(null);
    await navigate({ to: "/pair/$code", params: { code: target.code }, hash: target.fp ? `fp=${encodeURIComponent(target.fp)}` : undefined });
  }

  return (
    <SettingsCard
      id="pair-desktop"
      className={SETTINGS_ANCHOR}
      title="Pair a desktop"
      footnote={empty ? undefined : "On the desktop, open Settings > Web app > Pair. It shows a link and a QR code. Scan the code with this device, or paste the link here."}
    >
      <form onSubmit={(event) => void submit(event)} className="flex flex-col gap-3 px-1.5 py-2" noValidate>
        <Field
          label="Paste a pairing link"
          placeholder="Pairing link or code"
          value={value}
          onChange={(event) => {
            setValue(event.target.value);
            setError(null);
          }}
          autoComplete="off"
          autoCapitalize="none"
          spellCheck={false}
          error={error}
          hint="Paste the link or the 8 character code shown on the desktop."
        />
        <Button type="submit" className="w-full sm:w-auto sm:self-start" disabled={value.trim() === ""}>
          Continue
        </Button>
      </form>
    </SettingsCard>
  );
}

/**
 * `/pair`: "Pair a desktop" outside the settings shell, for an account with no desktop yet (where leaving settings goes,
 * see features/settings/settingsExit.tsx). The steps, then the paste field that continues to /pair/$code.
 */
export function PairDesktopPage() {
  return (
    <SettingsLayout>
      <PageHeader title="Pair a desktop" description="Link a desktop to this account to reach its sessions from this device." />
      <SettingsCard title="How it works">
        <div className="px-1.5 py-2">
          <PairStepsList />
        </div>
      </SettingsCard>
      <PairSection empty />
    </SettingsLayout>
  );
}
