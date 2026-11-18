// Ported from mobile/src/features/models/AccountPicker.tsx (F-54): the selected account when folded,
// one row per account with its usage meters when open, a refresh button for a fresh probe.
import { useState } from "react";
import { cn } from "@/lib/utils";
import { Check, ChevronDown, ChevronRight, LoaderCircle, RefreshCw } from "@/ui/icons";
import type { AccountUsage, AccountUsageResult } from "./hooks";

export interface PickerAccount {
  id: string;
  label: string;
}

type Snapshot = AccountUsageResult["accounts"][string];

export interface AccountPickerProps {
  accounts: readonly PickerAccount[];
  value: string;
  /** A running session: switching accounts opens a new chat. */
  existingSession: boolean;
  onPick: (accountId: string) => void;
  usage: Pick<AccountUsage, "accounts" | "refreshing" | "error" | "refresh">;
  /** Refresh is a control request; a read-only device cannot probe. */
  canRefresh?: boolean;
}

export function AccountPicker({ accounts, value, existingSession, onPick, usage, canRefresh = true }: AccountPickerProps) {
  const [open, setOpen] = useState(false);
  const selected = accounts.find((account) => account.id === value);
  const failed = usage.error != null;
  return (
    <div className="rounded-card bg-fill px-3.5" data-account-picker>
      <div className="flex items-center gap-1">
        <button
          type="button"
          aria-expanded={open}
          aria-label={`Account: ${selected?.label ?? "Account unavailable"}`}
          onClick={() => setOpen((current) => !current)}
          className="flex min-h-11 min-w-0 flex-1 items-center gap-2 text-left text-row font-medium text-content"
        >
          <span className="min-w-0 truncate">{selected?.label ?? "Unavailable"}</span>
          <AccountWarning snapshot={selected ? usage.accounts[selected.id] : undefined} />
          {open ? <ChevronDown size={14} className="shrink-0 text-muted-text" /> : <ChevronRight size={14} className="shrink-0 text-muted-text" />}
        </button>
        {canRefresh ? (
          <button
            type="button"
            aria-label="Refresh account usage"
            aria-busy={usage.refreshing}
            disabled={usage.refreshing}
            onClick={usage.refresh}
            className="-mr-2 inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection disabled:opacity-60"
          >
            {usage.refreshing ? <LoaderCircle size={17} className="animate-spin motion-reduce:animate-none" /> : <RefreshCw size={17} />}
          </button>
        ) : null}
      </div>
      {failed ? <p className="pb-2 text-meta text-muted-text">{usage.error}</p> : null}
      {open ? (
        <div role="radiogroup" aria-label="Account">
          {accounts.map((account) => {
            const checked = account.id === value;
            return (
              <button
                key={account.id}
                type="button"
                role="radio"
                aria-checked={checked}
                aria-label={account.label}
                onClick={() => {
                  if (checked) return;
                  onPick(account.id);
                  setOpen(false);
                }}
                className="flex w-full flex-col gap-1 border-t border-stroke py-2.5 text-left first:border-t-0"
              >
                <span className="flex items-center gap-2">
                  <span className={cn("min-w-0 truncate text-row text-content", checked && "font-semibold")}>{account.label}</span>
                  <AccountWarning snapshot={usage.accounts[account.id]} />
                  {checked ? <Check size={16} className="ml-auto shrink-0 text-brand-text" /> : null}
                </span>
                <AccountMeters snapshot={usage.accounts[account.id]} loading={usage.refreshing} failed={failed} />
              </button>
            );
          })}
          {existingSession || accounts.length < 2 ? (
            <p className="border-t border-stroke py-2 text-meta text-muted-text">
              {existingSession ? "Switching accounts opens a new chat." : "Add accounts in the desktop settings, under Accounts."}
            </p>
          ) : null}
        </div>
      ) : selected ? (
        <div className="pb-3">
          <AccountMeters snapshot={usage.accounts[selected.id]} loading={usage.refreshing} failed={failed} />
        </div>
      ) : null}
    </div>
  );
}

/** Healthy accounts need no status label; warnings stay beside the name. */
function AccountWarning({ snapshot }: { snapshot: Snapshot | undefined }) {
  const tone = snapshot?.status.tone;
  if (!snapshot || (tone !== "low" && tone !== "exhausted")) return null;
  return <span className={cn("shrink-0 text-meta font-medium", tone === "low" ? "text-caution" : "text-deletions")}>{snapshot.status.label}</span>;
}

/** A window, its reset countdown and the percentage share one line above the meter. */
function AccountMeters({ snapshot, loading, failed }: { snapshot: Snapshot | undefined; loading: boolean; failed: boolean }) {
  if (!snapshot || snapshot.meters.length === 0) {
    return <span className="text-meta text-muted-text">{snapshot?.status.label ?? (loading ? "Checking usage..." : failed ? "Usage unavailable" : "No usage data")}</span>;
  }
  return (
    <span className="flex flex-col gap-2">
      {snapshot.meters.map((meter) => {
        const tone = meter.usedPercent >= 90 ? "bg-deletions" : meter.usedPercent >= 80 ? "bg-caution" : "bg-muted-text";
        const text = meter.usedPercent >= 90 ? "text-deletions" : meter.usedPercent >= 80 ? "text-caution" : "text-muted-text";
        const reset = /^Resets in |^reset due/.test(meter.reset) ? ` · ${meter.reset.replace(/^Resets in /, "")}` : "";
        return (
          <span key={meter.title} className="flex flex-col gap-1">
            <span className="flex items-baseline justify-between gap-3 text-meta">
              <span className="min-w-0 truncate text-muted-text" title={`${meter.title}, ${meter.reset}`}>
                {meter.title}
                {reset}
              </span>
              <span className={text}>{meter.label}</span>
            </span>
            <span
              role="progressbar"
              aria-label={`${meter.title} usage`}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={meter.percent}
              aria-valuetext={meter.label}
              className="block h-1.5 w-full overflow-hidden rounded-pill bg-selection"
            >
              <span className={cn("block h-full rounded-pill", tone)} style={{ width: `${Math.min(100, Math.max(0, meter.percent))}%` }} />
            </span>
          </span>
        );
      })}
      {snapshot.stale && !failed ? <span className="text-meta text-muted-text">Last known usage</span> : null}
    </span>
  );
}
