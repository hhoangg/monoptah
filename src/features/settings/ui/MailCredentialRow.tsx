import { useState, type FormEvent } from "react";
import {
  mailCredentialDelete,
  mailCredentialUpsert,
} from "../../mail/model/mail";
import type { MailCredential } from "../../mail/model/mailTypes";
import { Loader, Trash2 } from "../../../shared/ui/icons";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";

export const FIELD_CLASS =
  "min-w-0 rounded-md border border-content/10 bg-content/5 px-2 py-1 text-[12px] text-content outline-none placeholder:text-content/30 focus:border-accent/60";
export const PILL_CLASS =
  "rounded-full border border-content/10 bg-content/5 px-1.5 text-[10px] text-content/70";

/** Tauri rejects with the Rust `String`, not an `Error`. */
export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : "Something went wrong.";
}

/**
 * Adds a key pair or rotates one. The backend keys credentials by label, so a
 * rotation is an upsert with the same label and the label field is locked.
 * The secret lives only in this component's state: it is never initialised from
 * server data (there is none) and is cleared as soon as the save succeeds.
 */
export function CredentialEditor({
  existing,
  onSaved,
  onCancel,
}: {
  existing?: MailCredential;
  onSaved: () => void;
  onCancel?: () => void;
}) {
  const [label, setLabel] = useState(existing?.label ?? "");
  const [accessKeyId, setAccessKeyId] = useState("");
  const [secret, setSecret] = useState("");
  const [region, setRegion] = useState(existing?.defaultRegion ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await mailCredentialUpsert({
        label,
        accessKeyId,
        secretAccessKey: secret,
        defaultRegion: region.trim() || null,
      });
      setAccessKeyId("");
      setSecret("");
      if (!existing) setLabel("");
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setSecret("");
      setBusy(false);
    }
  };

  return (
    <form
      onSubmit={submit}
      className="flex flex-wrap items-center gap-2 px-4 py-3"
    >
      <input
        aria-label="Credential label"
        className={`${FIELD_CLASS} w-32`}
        placeholder="label"
        value={label}
        readOnly={Boolean(existing)}
        onChange={(event) => setLabel(event.target.value)}
      />
      <input
        aria-label="AWS access key id"
        className={`${FIELD_CLASS} w-44 font-mono`}
        placeholder="AKIA…"
        autoComplete="off"
        spellCheck={false}
        value={accessKeyId}
        onChange={(event) => setAccessKeyId(event.target.value)}
      />
      <input
        aria-label="AWS secret access key"
        className={`${FIELD_CLASS} w-44 font-mono`}
        type="password"
        placeholder="secret…"
        autoComplete="new-password"
        value={secret}
        onChange={(event) => setSecret(event.target.value)}
      />
      <input
        aria-label="Default region"
        className={`${FIELD_CLASS} w-28`}
        placeholder="region"
        value={region}
        onChange={(event) => setRegion(event.target.value)}
      />
      <SecondaryButton
        type="submit"
        disabled={busy || !label.trim() || !accessKeyId.trim() || !secret}
      >
        {busy ? <Loader className="size-3 animate-spin" /> : null}
        {existing ? "Save key" : "Add"}
      </SecondaryButton>
      {onCancel ? (
        <SecondaryButton onClick={onCancel}>Cancel</SecondaryButton>
      ) : null}
      {error ? (
        <p role="alert" className="basis-full text-[12px] text-red-400">
          {error}
        </p>
      ) : null}
    </form>
  );
}

export function MailCredentialRow({
  credential,
  usedBy,
  onChanged,
}: {
  credential: MailCredential;
  /** Labels of the sources that reference this credential. */
  usedBy: string[];
  onChanged: () => void;
}) {
  const [rotating, setRotating] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const remove = async () => {
    setBusy(true);
    setError(null);
    try {
      await mailCredentialDelete(credential.id);
      onChanged();
    } catch (err) {
      // The backend names the sources still holding the key; that is the only
      // way the user learns which one to detach first.
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="border-b border-content/5 last:border-b-0">
      <div
        data-credential-id={credential.id}
        className="flex items-center gap-3 px-4 py-2.5 text-[12px]"
      >
        <span className="w-32 min-w-0 truncate text-[13px] font-medium text-content">
          {credential.label}
        </span>
        <span className="min-w-0 flex-1 font-mono text-content/70">
          {credential.accessKeyMasked}
        </span>
        <span className="w-24 text-content/50" title={usedBy.join(", ")}>
          {usedBy.length === 0
            ? "Not used"
            : `${usedBy.length} ${usedBy.length === 1 ? "source" : "sources"}`}
        </span>
        <SecondaryButton onClick={() => setRotating((prev) => !prev)}>
          Rotate
        </SecondaryButton>
        <SecondaryButton
          danger
          disabled={busy}
          aria-label={`Delete credential ${credential.label}`}
          onClick={remove}
        >
          <Trash2 className="size-3.5" />
        </SecondaryButton>
      </div>
      {error ? (
        <p role="alert" className="px-4 pb-2.5 text-[12px] text-red-400">
          {error}
        </p>
      ) : null}
      {rotating ? (
        <CredentialEditor
          existing={credential}
          onCancel={() => setRotating(false)}
          onSaved={() => {
            setRotating(false);
            onChanged();
          }}
        />
      ) : null}
    </div>
  );
}
