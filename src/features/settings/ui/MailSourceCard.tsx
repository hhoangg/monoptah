import { useState, type FormEvent, type ReactNode } from "react";
import {
  mailMailboxesDelete,
  mailMailboxesUpsert,
  mailSourceDelete,
  mailSourceDetectPrefixes,
  mailSourceTest,
  mailSourceUpsert,
  mailSyncNow,
} from "../../mail/model/mail";
import type {
  CheckResult,
  IdentityRow,
  IdentityStatus,
  MailCredential,
  MailMailbox,
  SourceRow,
  SourceStatus,
  SyncReport,
  TestReport,
} from "../../mail/model/mailTypes";
import { Loader, Trash2 } from "../../../shared/ui/icons";
import { SearchableSelect } from "../../../shared/ui/SearchableSelect";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { Toggle } from "./primitives";
import {
  errorMessage,
  FIELD_CLASS,
  PILL_CLASS,
  RegionSelect,
} from "./MailCredentialRow";

const SAME_AS_SES = "";

/** Each bucket needs its own resource ARN, so the policy is built per source. */
export function iamPolicy(bucket: string): string {
  const arn = `arn:aws:s3:::${bucket}`;
  return JSON.stringify(
    {
      Version: "2012-10-17",
      Statement: [
        {
          Sid: "ListMailBucket",
          Effect: "Allow",
          Action: "s3:ListBucket",
          Resource: arn,
        },
        {
          Sid: "ReadWriteMail",
          Effect: "Allow",
          Action: ["s3:GetObject", "s3:PutObject"],
          Resource: `${arn}/*`,
        },
        {
          // Only needed for "Delete on S3 as well".
          Sid: "HardDeleteOptional",
          Effect: "Allow",
          Action: "s3:DeleteObject",
          Resource: `${arn}/*`,
        },
        {
          Sid: "Ses",
          Effect: "Allow",
          Action: [
            "ses:SendEmail",
            "ses:ListEmailIdentities",
            "ses:GetAccount",
          ],
          Resource: "*",
        },
      ],
    },
    null,
    2,
  );
}

/** `ms` is epoch milliseconds (sync timestamps), not seconds. */
function ago(ms: number): string {
  const minutes = Math.max(0, Math.round((Date.now() - ms) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  return hours < 48 ? `${hours}h ago` : `${Math.round(hours / 24)}d ago`;
}

/** Domain is a display concern, read off the address. There is no domain table. */
function domainOf(address: string): string {
  return address.slice(address.lastIndexOf("@") + 1).toLowerCase();
}

function groupByDomain(mailboxes: MailMailbox[]): [string, MailMailbox[]][] {
  const groups = new Map<string, MailMailbox[]>();
  for (const mailbox of mailboxes) {
    const domain = domainOf(mailbox.address);
    groups.set(domain, [...(groups.get(domain) ?? []), mailbox]);
  }
  return [...groups];
}

const IDENTITY_HINT: Record<IdentityStatus, string> = {
  verified: "Verified: mail can be sent from this identity.",
  pending:
    "Waiting for AWS to verify it. Check the DNS records, then test again.",
  failed: "AWS could not verify it. Check the DNS records for this identity.",
  sending_disabled:
    "AWS verified this identity, but sending is switched off for the whole AWS account in this region. This is not a verification problem and re-verifying will not help. Enable sending in the SES console for this account, then test again.",
};

const IDENTITY_LABEL: Record<IdentityStatus, string> = {
  verified: "verified",
  pending: "pending",
  failed: "failed",
  sending_disabled: "sending disabled",
};

function CheckLine({ name, check }: { name: string; check: CheckResult }) {
  return (
    <li className="flex gap-2 text-[12px]">
      <span className={check.ok ? "text-content/70" : "text-red-400"}>
        {check.ok ? "OK" : "Failed"}
      </span>
      <span className="text-content/70">{name}</span>
      <span className="font-mono text-content/45">{check.action}</span>
      {!check.ok ? (
        <span className="min-w-0 text-red-400">
          {[check.kind, check.message].filter(Boolean).join(": ")}
        </span>
      ) : null}
    </li>
  );
}

function FieldLabel({ text, children }: { text: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1 text-[11px] text-content/50">
      {text}
      {children}
    </label>
  );
}

export function MailSourceCard({
  source,
  credentials,
  identities,
  status,
  onChanged,
}: {
  source: SourceRow;
  credentials: MailCredential[];
  identities: IdentityRow[];
  /** Carries `lastError`; `mail_sources` does not. */
  status: SourceStatus | undefined;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState<null | "test" | "sync" | "detect">(null);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<TestReport | null>(null);
  const [sync, setSync] = useState<SyncReport | null>(null);
  const [found, setFound] = useState<string[] | null>(null);
  const [address, setAddress] = useState("");
  const [prefix, setPrefix] = useState("");

  const credential = credentials.find((c) => c.id === source.credentialId);
  const s3Credential = credentials.find((c) => c.id === source.s3CredentialId);
  const lastError = status?.lastError ?? null;
  // AWS verified these but sending is off account-wide; saying "not verified"
  // would send the user to DNS for a problem that lives in the SES console.
  const sendingDisabled = (address: string) =>
    identities.some(
      (i) =>
        i.status === "sending_disabled" &&
        (i.identity === address.toLowerCase() ||
          i.identity === domainOf(address)),
    );
  const domains = groupByDomain(source.mailboxes);
  const mapped = new Set(source.mailboxes.map((m) => m.s3Prefix));

  const run = async (action: () => Promise<void>) => {
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(null);
    }
  };

  const test = () => {
    setBusy("test");
    return run(async () => {
      setReport(await mailSourceTest(source.id));
      // The test replaces the cached identities and re-resolves is_sendable.
      onChanged();
    });
  };
  const syncNow = () => {
    setBusy("sync");
    return run(async () => {
      const reports = await mailSyncNow({ kind: "source", id: source.id });
      setSync(reports[0] ?? null);
      onChanged();
    });
  };
  const detect = () => {
    setBusy("detect");
    return run(async () => setFound(await mailSourceDetectPrefixes(source.id)));
  };
  const addMailbox = (event: FormEvent) => {
    event.preventDefault();
    const existing = source.mailboxes.find(
      (m) => m.address.toLowerCase() === address.trim().toLowerCase(),
    );
    return run(async () => {
      await mailMailboxesUpsert(source.id, [
        {
          address: address.trim(),
          s3Prefix: prefix.trim(),
          // Re-adding an address upserts every column, so carry over what the
          // user may have set rather than overwrite it with empties.
          displayName: existing?.displayName ?? "",
          color: existing?.color ?? null,
          sortOrder: existing?.sortOrder ?? source.mailboxes.length,
        },
      ]);
      setAddress("");
      setPrefix("");
      onChanged();
    });
  };
  const removeMailbox = (id: number) =>
    run(async () => {
      await mailMailboxesDelete(id);
      onChanged();
    });
  const removeSource = () =>
    run(async () => {
      await mailSourceDelete(source.id);
      onChanged();
    });

  return (
    <div
      data-source-id={source.id}
      className="border-b border-content/5 px-4 py-3.5 last:border-b-0"
    >
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2 text-[13px] font-medium text-content">
            {source.label}
            <span className="font-normal text-content/45">
              {source.bucket} · {source.sesRegion}
            </span>
            {!source.enabled ? (
              <span className={PILL_CLASS}>paused</span>
            ) : null}
            {lastError ? (
              <span className="rounded-full border border-red-400/40 bg-red-400/10 px-1.5 text-[10px] text-red-400">
                error
              </span>
            ) : status?.lastSyncAt ? (
              <span className={PILL_CLASS}>OK · {ago(status.lastSyncAt)}</span>
            ) : null}
          </div>
          <p className="mt-0.5 text-[12px] text-content/45">
            key {credential?.label ?? "missing"} ·{" "}
            {domains.map(([domain]) => domain).join(", ") || "no domains yet"} ·{" "}
            {source.mailboxes.length}{" "}
            {source.mailboxes.length === 1 ? "mailbox" : "mailboxes"}
          </p>
        </div>
        <SecondaryButton onClick={test} disabled={busy !== null}>
          {busy === "test" ? <Loader className="size-3 animate-spin" /> : null}
          Test
        </SecondaryButton>
        <SecondaryButton
          onClick={syncNow}
          disabled={busy !== null}
          title="Syncs every mailbox in this source"
        >
          {busy === "sync" ? <Loader className="size-3 animate-spin" /> : null}
          Sync now
        </SecondaryButton>
        <SecondaryButton onClick={() => setEditing((prev) => !prev)}>
          {editing ? "Close" : "Edit"}
        </SecondaryButton>
      </div>

      {lastError ? (
        <div
          role="alert"
          data-testid="source-error"
          className="mt-2.5 rounded-md border border-red-400/30 bg-red-400/10 px-3 py-2 text-[12px] text-red-400"
        >
          <p>{lastError}</p>
          <p className="mt-1 text-content/60">
            Key {credential?.label ?? "this source's key"} needs the permissions
            listed under IAM policy below, on arn:aws:s3:::{source.bucket}.
            Other sources are not affected.
          </p>
        </div>
      ) : null}

      {error ? (
        <p role="alert" className="mt-2 text-[12px] text-red-400">
          {error}
        </p>
      ) : null}

      {report ? (
        <ul
          className="mt-2.5 space-y-1"
          aria-label={`Test result for ${source.label}`}
        >
          <CheckLine name="S3 read" check={report.s3} />
          <CheckLine name="SES identities" check={report.sesIdentities} />
          <CheckLine name="SES account" check={report.sesAccount} />
          {report.sandbox ? (
            <li className="text-[12px] text-content/50">
              SES is in sandbox mode: it only delivers to verified addresses.
            </li>
          ) : null}
        </ul>
      ) : null}

      {sync ? (
        <p className="mt-2 text-[12px] text-content/50">
          {sync.error
            ? `Sync failed: ${sync.error}`
            : `Synced: ${sync.inserted} new, ${sync.archived} archived, ${sync.failed} failed.`}
        </p>
      ) : null}

      {editing ? (
        <SourceForm
          source={source}
          credentials={credentials}
          onSaved={() => {
            setEditing(false);
            onChanged();
          }}
        />
      ) : (
        <p className="mt-2 text-[12px] text-content/45">
          SES {source.sesRegion} · S3 {source.s3Region} · S3 key{" "}
          {source.s3CredentialId == null
            ? "Same as SES"
            : (s3Credential?.label ?? "missing")}
          {source.defaultFrom ? ` · From ${source.defaultFrom}` : ""}
        </p>
      )}

      <div className="mt-3">
        <div className="flex items-center gap-2">
          <h3 className="text-[12px] font-semibold text-content">Mailboxes</h3>
          <span className="text-[11px] text-content/45">
            one row per SES receipt rule
          </span>
          <div className="flex-1" />
          <SecondaryButton onClick={detect} disabled={busy !== null}>
            {busy === "detect" ? (
              <Loader className="size-3 animate-spin" />
            ) : null}
            Detect prefixes
          </SecondaryButton>
        </div>
        {domains.length === 0 ? (
          <p className="py-2 text-[12px] text-content/50">
            No mailboxes yet. Map an address to the S3 prefix its mail lands in.
          </p>
        ) : (
          domains.map(([domain, boxes]) => (
            <div key={domain} className="mt-2" data-domain={domain}>
              <div className="text-[11px] text-content/45">{domain}</div>
              {boxes.map((mailbox) => (
                <div
                  key={mailbox.id}
                  className="flex items-center gap-3 py-1 text-[12px]"
                >
                  <span className="w-56 truncate text-content">
                    {mailbox.address}
                  </span>
                  <span className="min-w-0 flex-1 truncate font-mono text-content/70">
                    {mailbox.s3Prefix || "(bucket root)"}
                  </span>
                  <span
                    className={PILL_CLASS}
                    title={
                      mailbox.isSendable
                        ? "This address or its domain is a verified SES identity."
                        : sendingDisabled(mailbox.address)
                          ? "AWS verified this identity, but sending is switched off for the whole AWS account in this region. Enable sending in the SES console; this is not a verification problem."
                          : "Not sendable: no verified SES identity covers this address in this source's account and region."
                    }
                  >
                    {mailbox.isSendable ? "can send" : "receive only"}
                  </span>
                  <SecondaryButton
                    danger
                    aria-label={`Remove mailbox ${mailbox.address}`}
                    onClick={() => removeMailbox(mailbox.id)}
                  >
                    <Trash2 className="size-3.5" />
                  </SecondaryButton>
                </div>
              ))}
            </div>
          ))
        )}
        {found ? (
          <div className="mt-2 flex flex-wrap items-center gap-1.5 text-[12px]">
            {found.length === 0 ? (
              <span className="text-content/50">
                No prefixes found in this bucket.
              </span>
            ) : (
              <>
                <span className="text-content/50">Found:</span>
                {found.map((candidate) => (
                  <button
                    key={candidate}
                    type="button"
                    disabled={mapped.has(candidate)}
                    title={
                      mapped.has(candidate)
                        ? "Already mapped to a mailbox"
                        : "Use as the prefix of the new mailbox"
                    }
                    onClick={() => setPrefix(candidate)}
                    className={`${PILL_CLASS} font-mono hover:bg-content/10 disabled:opacity-40`}
                  >
                    {candidate}
                  </button>
                ))}
              </>
            )}
          </div>
        ) : null}
        <form onSubmit={addMailbox} className="mt-2 flex items-center gap-2">
          <input
            aria-label={`New mailbox address for ${source.label}`}
            className={`${FIELD_CLASS} w-56`}
            placeholder="owner@example.com"
            value={address}
            onChange={(event) => setAddress(event.target.value)}
          />
          <input
            aria-label={`New mailbox S3 prefix for ${source.label}`}
            className={`${FIELD_CLASS} min-w-0 flex-1 font-mono`}
            placeholder="owner/"
            value={prefix}
            onChange={(event) => setPrefix(event.target.value)}
          />
          <SecondaryButton type="submit" disabled={!address.trim()}>
            Add
          </SecondaryButton>
        </form>
      </div>

      {identities.length > 0 ? (
        <div className="mt-3">
          <h3 className="text-[12px] font-semibold text-content">
            Verified identities
          </h3>
          <p className="text-[11px] text-content/45">
            Per AWS account and region, so a domain can be sendable here and not
            in another source.
          </p>
          <ul className="mt-1.5 space-y-1">
            {identities.map((identity) => (
              <li key={identity.identity} className="text-[12px]">
                <span className="text-content">{identity.identity}</span>{" "}
                <span className={PILL_CLASS}>
                  {IDENTITY_LABEL[identity.status]}
                </span>
                {identity.status !== "verified" ? (
                  <p className="mt-0.5 text-content/50">
                    {IDENTITY_HINT[identity.status]}
                  </p>
                ) : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}

      <details className="mt-3">
        <summary className="cursor-pointer text-[12px] text-content/70">
          IAM policy
        </summary>
        <p className="mt-1 text-[11px] text-content/45">
          The minimum permissions for this source, with its own bucket ARN. The
          delete statement is only needed for "Delete on S3 as well".
        </p>
        <pre className="mt-1.5 overflow-x-auto rounded-md border border-content/10 bg-content/5 p-2.5 font-mono text-[11px] text-content/70">
          {iamPolicy(source.bucket)}
        </pre>
      </details>

      <div className="mt-3 flex items-center justify-end gap-2 text-[12px]">
        {confirmDelete ? (
          <>
            <span className="text-content/50">
              Removes this source and its synced mail from this Mac. Nothing in
              S3 is deleted.
            </span>
            <SecondaryButton onClick={() => setConfirmDelete(false)}>
              Cancel
            </SecondaryButton>
            <SecondaryButton danger onClick={removeSource}>
              Delete source
            </SecondaryButton>
          </>
        ) : (
          <SecondaryButton danger onClick={() => setConfirmDelete(true)}>
            Delete source
          </SecondaryButton>
        )}
      </div>
    </div>
  );
}

export function SourceForm({
  source,
  credentials,
  onSaved,
  onCancel,
}: {
  source?: SourceRow;
  credentials: MailCredential[];
  onSaved: () => void;
  onCancel?: () => void;
}) {
  const [label, setLabel] = useState(source?.label ?? "");
  const [credentialId, setCredentialId] = useState(
    String(source?.credentialId ?? credentials[0]?.id ?? ""),
  );
  const [s3CredentialId, setS3CredentialId] = useState(
    source?.s3CredentialId == null
      ? SAME_AS_SES
      : String(source.s3CredentialId),
  );
  const [sesRegion, setSesRegion] = useState(source?.sesRegion ?? "us-east-1");
  const [bucket, setBucket] = useState(source?.bucket ?? "");
  const [s3Region, setS3Region] = useState(source?.s3Region ?? "us-east-1");
  const [defaultFrom, setDefaultFrom] = useState(source?.defaultFrom ?? "");
  const [enabled, setEnabled] = useState(source?.enabled ?? true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const options = credentials.map((c) => ({
    value: String(c.id),
    label: `${c.label} ${c.accessKeyMasked}`,
  }));

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await mailSourceUpsert({
        id: source?.id ?? null,
        provider: source?.provider ?? "aws",
        credentialId: Number(credentialId),
        s3CredentialId:
          s3CredentialId === SAME_AS_SES ? null : Number(s3CredentialId),
        label,
        sesRegion: sesRegion.trim(),
        bucket: bucket.trim(),
        s3Region: s3Region.trim(),
        defaultFrom: defaultFrom.trim() || null,
        attachPolicy: source?.attachPolicy ?? "on_demand",
        imagePolicy: source?.imagePolicy ?? "ask",
        enabled,
        sortOrder: source?.sortOrder ?? 0,
      });
      onSaved();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      onSubmit={submit}
      aria-label={source ? `Edit ${source.label}` : "Add source"}
      className="mt-3 grid grid-cols-2 gap-3 rounded-md border border-content/10 p-3"
    >
      <FieldLabel text="Name">
        <input
          aria-label="Source name"
          className={FIELD_CLASS}
          value={label}
          onChange={(event) => setLabel(event.target.value)}
        />
      </FieldLabel>
      <FieldLabel text="Credential">
        <SearchableSelect
          label="Credential"
          value={credentialId}
          options={options}
          onChange={setCredentialId}
          placeholder="Add a key pair first"
          searchable={false}
        />
      </FieldLabel>
      <FieldLabel text="SES region">
        <RegionSelect
          label="SES region"
          value={sesRegion}
          onChange={setSesRegion}
        />
      </FieldLabel>
      <FieldLabel text="Bucket">
        <input
          aria-label="Bucket"
          className={FIELD_CLASS}
          value={bucket}
          onChange={(event) => setBucket(event.target.value)}
        />
      </FieldLabel>
      <FieldLabel text="S3 region">
        <RegionSelect
          label="S3 region"
          value={s3Region}
          onChange={setS3Region}
        />
      </FieldLabel>
      <FieldLabel text="S3 credential">
        <SearchableSelect
          label="S3 credential"
          value={s3CredentialId}
          options={[{ value: SAME_AS_SES, label: "Same as SES" }, ...options]}
          onChange={setS3CredentialId}
          searchable={false}
        />
      </FieldLabel>
      <FieldLabel text="Default From">
        <input
          aria-label="Default from address"
          className={FIELD_CLASS}
          placeholder="Name <owner@example.com>"
          value={defaultFrom}
          onChange={(event) => setDefaultFrom(event.target.value)}
        />
      </FieldLabel>
      <div className="flex items-end gap-2 text-[12px] text-content/70">
        <Toggle label="Source enabled" on={enabled} onChange={setEnabled} />
        {enabled ? "Syncing" : "Paused"}
      </div>
      {error ? (
        <p role="alert" className="col-span-2 text-[12px] text-red-400">
          {error}
        </p>
      ) : null}
      <div className="col-span-2 flex justify-end gap-2">
        {onCancel ? (
          <SecondaryButton onClick={onCancel}>Cancel</SecondaryButton>
        ) : null}
        <SecondaryButton
          type="submit"
          disabled={busy || !label.trim() || !bucket.trim() || !credentialId}
        >
          {busy ? <Loader className="size-3 animate-spin" /> : null}
          {source ? "Save source" : "Add source"}
        </SecondaryButton>
      </div>
    </form>
  );
}
