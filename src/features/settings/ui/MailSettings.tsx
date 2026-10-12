import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import {
  mailCredentials,
  mailIdentities,
  mailSources,
  mailSourceUpsert,
  mailStatus,
  mailSyncIntervalSet,
} from "../../mail/model/mail";
import {
  density,
  readingPaneSide,
  syncIntervalSeconds,
  SYNC_INTERVAL_MIN_SECONDS,
  type MailDensity,
  type ReadingPaneSide,
} from "../../mail/model/mailPrefs";
import type {
  ImagePolicy,
  IdentityRow,
  MailCredential,
  MailStatus,
  SourceRow,
} from "../../mail/model/mailTypes";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import {
  loadMailEnabled,
  saveMailEnabled,
  subscribeMailEnabled,
} from "../model/settings";
import {
  CredentialEditor,
  errorMessage,
  MailCredentialRow,
} from "./MailCredentialRow";
import { MailSourceCard, SourceForm } from "./MailSourceCard";
import { Group, Row, Segmented, Select, Toggle } from "./primitives";

const INTERVALS: [number, string][] = [
  [60, "Every minute"],
  [300, "Every 5 minutes"],
  [900, "Every 15 minutes"],
  [1800, "Every 30 minutes"],
  [3600, "Every hour"],
  [21600, "Every 6 hours"],
  [86400, "Every day"],
];

const IMAGE_OPTIONS: { value: ImagePolicy; label: string }[] = [
  { value: "ask", label: "Ask each time" },
  { value: "always", label: "Always load" },
  { value: "never", label: "Never load" },
];

const DENSITY_OPTIONS: { value: MailDensity; label: string }[] = [
  { value: "comfortable", label: "Comfortable" },
  { value: "compact", label: "Compact" },
];

const PANE_OPTIONS: { value: ReadingPaneSide; label: string }[] = [
  { value: "right", label: "Right" },
  { value: "bottom", label: "Bottom" },
  { value: "off", label: "Off" },
];

type Loaded = {
  credentials: MailCredential[];
  sources: SourceRow[];
  identities: IdentityRow[];
  status: MailStatus | null;
};

export function MailSettings() {
  const enabled = useSyncExternalStore(
    subscribeMailEnabled,
    loadMailEnabled,
    () => false,
  );
  const interval = syncIntervalSeconds.useValue();
  const rowDensity = density.useValue();
  const paneSide = readingPaneSide.useValue();
  const [data, setData] = useState<Loaded>({
    credentials: [],
    sources: [],
    identities: [],
    status: null,
  });
  const [loadError, setLoadError] = useState<string | null>(null);
  const [intervalError, setIntervalError] = useState<string | null>(null);
  const [addingSource, setAddingSource] = useState(false);
  const [version, setVersion] = useState(0);
  const refresh = useCallback(() => setVersion((v) => v + 1), []);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      mailCredentials(),
      mailSources(),
      mailIdentities(),
      mailStatus(),
    ])
      .then(([credentials, sources, identities, status]) => {
        if (cancelled) return;
        setLoadError(null);
        setData({
          credentials: credentials ?? [],
          sources: sources ?? [],
          identities: identities ?? [],
          status: status ?? null,
        });
      })
      .catch((err) => {
        if (!cancelled) setLoadError(errorMessage(err));
      });
    return () => {
      cancelled = true;
    };
  }, [version]);

  const { credentials, sources, identities, status } = data;
  const usedBy = (id: number) =>
    sources
      .filter((s) => s.credentialId === id || s.s3CredentialId === id)
      .map((s) => s.label);

  // The policy is stored per source and Rust enforces it, so this control is
  // a shortcut that writes the same value to every source.
  const policies = new Set(sources.map((s) => s.imagePolicy));
  const imagesMixed = policies.size > 1;
  const images: string = imagesMixed
    ? "mixed"
    : (sources[0]?.imagePolicy ?? "ask");
  const [imagesError, setImagesError] = useState<string | null>(null);
  const changeImages = async (policy: ImagePolicy) => {
    setImagesError(null);
    try {
      for (const s of sources) {
        await mailSourceUpsert({
          id: s.id,
          provider: s.provider,
          credentialId: s.credentialId,
          s3CredentialId: s.s3CredentialId,
          label: s.label,
          sesRegion: s.sesRegion,
          bucket: s.bucket,
          s3Region: s.s3Region,
          defaultFrom: s.defaultFrom,
          attachPolicy: s.attachPolicy,
          imagePolicy: policy,
          enabled: s.enabled,
          sortOrder: s.sortOrder,
        });
      }
    } catch (err) {
      setImagesError(errorMessage(err));
    }
    refresh();
  };

  const changeInterval = (secs: number) => {
    syncIntervalSeconds.save(secs);
    setIntervalError(null);
    // Rust cannot read localStorage; this push is the only way the scheduler
    // learns the new value.
    mailSyncIntervalSet(secs).catch((err) =>
      setIntervalError(errorMessage(err)),
    );
  };
  const intervalOptions = INTERVALS.some(([secs]) => secs === interval)
    ? INTERVALS
    : [
        ...INTERVALS,
        [interval, `Every ${interval} seconds`] as [number, string],
      ];

  return (
    <>
      <Group title="Mail">
        <Row
          id="mail-enabled"
          label="Enable Mail"
          description="Show Mail in the sidebar. Off removes it and stops the background sync."
        >
          <Toggle label="Enable Mail" on={enabled} onChange={saveMailEnabled} />
        </Row>
      </Group>

      <Group
        title="Providers"
        description="Monoptah reads mail from the S3 buckets SES writes into, and sends through SES. Secrets are stored in the system keychain and only used by the Rust side of the app."
      >
        <Row
          label="Amazon SES + S3"
          description={`${credentials.length} key ${credentials.length === 1 ? "pair" : "pairs"} · ${sources.length} ${sources.length === 1 ? "source" : "sources"}`}
        />
        <Row label="IMAP + SMTP" description="Planned.">
          <span className="text-[11px] text-content/45">Soon</span>
        </Row>
        <Row label="Cloudflare Email Routing" description="Planned.">
          <span className="text-[11px] text-content/45">Soon</span>
        </Row>
      </Group>

      {loadError ? (
        <p role="alert" className="pt-4 text-[12px] text-red-400">
          {loadError}
        </p>
      ) : null}

      <Group
        id="mail-credentials"
        title="AWS credentials"
        description="One entry per AWS account. Several domains in the same account share one entry, so rotating a key is one edit. A secret goes to the keychain and is never shown again."
      >
        {credentials.length === 0 ? (
          <p className="px-4 py-3 text-[12px] text-content/50">
            No key pairs yet. Add one below.
          </p>
        ) : (
          credentials.map((credential) => (
            <MailCredentialRow
              key={credential.id}
              credential={credential}
              usedBy={usedBy(credential.id)}
              onChanged={refresh}
            />
          ))
        )}
        <CredentialEditor onSaved={refresh} />
      </Group>

      <Group
        id="mail-sources"
        title="Mail sources"
        description="One per bucket and SES region, each with its own key and prefixes. A source can serve several domains. Two AWS accounts are simply two sources."
        action={
          <SecondaryButton onClick={() => setAddingSource((prev) => !prev)}>
            Add source
          </SecondaryButton>
        }
      >
        {addingSource ? (
          <div className="px-4 pb-3.5">
            <SourceForm
              credentials={credentials}
              onCancel={() => setAddingSource(false)}
              onSaved={() => {
                setAddingSource(false);
                refresh();
              }}
            />
          </div>
        ) : null}
        {sources.length === 0 && !addingSource ? (
          <p className="px-4 py-3 text-[12px] text-content/50">
            No sources yet. Add a key pair, then point a source at its bucket.
          </p>
        ) : null}
        {sources.map((source) => (
          <MailSourceCard
            key={source.id}
            source={source}
            credentials={credentials}
            identities={identities.filter((i) => i.accountId === source.id)}
            status={status?.sources.find((s) => s.id === source.id)}
            onChanged={refresh}
          />
        ))}
      </Group>

      <Group
        title="Defaults"
        description="Apply to every source. There is one sync interval for the whole app."
      >
        <Row
          id="mail-sync-interval"
          label="Sync interval"
          description="How often each bucket is listed and reconciled with the local copy."
        >
          <Select
            label="Sync interval"
            value={String(interval)}
            options={intervalOptions
              .filter(([secs]) => secs >= SYNC_INTERVAL_MIN_SECONDS)
              .map(([secs, text]) => ({ value: String(secs), label: text }))}
            onChange={(value) => changeInterval(Number(value))}
          />
          {intervalError ? (
            <p
              role="alert"
              className="basis-full text-right text-[12px] text-red-400"
            >
              {intervalError}
            </p>
          ) : null}
        </Row>
        <Row
          id="mail-show-images"
          label="Load remote images"
          description="Loading an image still tells the sender you opened the message and from which IP address. Monoptah fetches it itself, so there are no cookies, no Referer, and addresses on your own network are refused. Applies to every source."
        >
          <Select
            label="Load remote images"
            value={images}
            options={
              imagesMixed
                ? [
                    { value: "mixed", label: "Set per source" },
                    ...IMAGE_OPTIONS,
                  ]
                : IMAGE_OPTIONS
            }
            onChange={(value) => {
              if (value !== "mixed") void changeImages(value as ImagePolicy);
            }}
          />
          {imagesError ? (
            <p
              role="alert"
              className="basis-full text-right text-[12px] text-red-400"
            >
              {imagesError}
            </p>
          ) : null}
        </Row>
      </Group>

      <Group
        title="Layout"
        description="How the Mail view is laid out. Saved on this computer."
      >
        <Row
          id="mail-density"
          label="List density"
          description="Compact hides the message preview and tightens each row."
        >
          <Segmented<MailDensity>
            label="List density"
            value={rowDensity}
            options={DENSITY_OPTIONS}
            onChange={density.save}
          />
        </Row>
        <Row
          id="mail-reading-pane"
          label="Reading pane"
          description="Where a message opens. Off opens it in place of the list."
        >
          <Segmented<ReadingPaneSide>
            label="Reading pane"
            value={paneSide}
            options={PANE_OPTIONS}
            onChange={readingPaneSide.save}
          />
        </Row>
      </Group>
    </>
  );
}
