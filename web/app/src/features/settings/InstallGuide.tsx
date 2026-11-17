import { useMemo } from "react";
import { Icon, type IconName } from "@/ui/Icon";
import { PageHeader } from "@/ui/PageHeader";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { PushOnboarding } from "./PushOnboarding";
import { SettingsLayout } from "./SettingsLayout";
import { detectPlatform, useStandalone, type InstallPlatform } from "./standalone";

interface Step {
  icon: IconName;
  text: string;
}

interface Guide {
  platform: InstallPlatform;
  title: string;
  icon: IconName;
  steps: Step[];
  footnote?: string;
}

// Icon-based illustrations: each step pairs the real browser control with its Hugeicons glyph.
const GUIDES: Guide[] = [
  {
    platform: "ios",
    title: "iPhone and iPad (Safari)",
    icon: "smartphone",
    steps: [
      { icon: "share", text: "Tap the Share button in the Safari toolbar." },
      { icon: "addSquare", text: "Scroll down and choose Add to Home Screen." },
      { icon: "check", text: "Tap Add. Open Monoptah from your home screen." },
    ],
    footnote: "On iOS the app can only be installed from Safari, and notifications work only from the installed app.",
  },
  {
    platform: "android",
    title: "Android (Chrome)",
    icon: "smartphone",
    steps: [
      { icon: "moreVertical", text: "Open the browser menu with the three dots." },
      { icon: "download", text: "Choose Install app (or Add to Home screen)." },
      { icon: "check", text: "Confirm. Open Monoptah from your app list." },
    ],
  },
  {
    platform: "desktop",
    title: "Computer (Chrome or Edge)",
    icon: "laptop",
    steps: [
      { icon: "download", text: "Click the install icon at the right end of the address bar." },
      { icon: "check", text: "Choose Install in the prompt that opens." },
      { icon: "computer", text: "Open Monoptah from your applications or the taskbar." },
    ],
    footnote: "Safari on a Mac: File > Add to Dock. Firefox does not install web apps.",
  },
];

function GuideCard({ guide, current }: { guide: Guide; current: boolean }) {
  return (
    <SettingsCard
      title={current ? `${guide.title} (this device)` : guide.title}
      footnote={guide.footnote}
      aria-label={guide.title}
      data-current={current ? "true" : undefined}
    >
      <ol className="m-0 flex list-none flex-col p-0">
        {guide.steps.map((step, index) => (
          <li key={step.text} className="relative flex min-h-13 items-center gap-3 px-1.5 py-2 text-row text-content before:absolute before:top-0 before:right-0 before:left-4 before:h-px before:bg-stroke first:before:hidden">
            <span className="inline-flex size-11 shrink-0 items-center justify-center rounded-row bg-fill text-content">
              <Icon name={step.icon} size={22} />
            </span>
            <span className="min-w-0 flex-1">
              <span className="text-meta text-muted-text tabular">Step {index + 1}</span>
              <span className="block">{step.text}</span>
            </span>
          </li>
        ))}
      </ol>
    </SettingsCard>
  );
}

/** /install (N-13): per-platform guide, with the guide for this device first and an Installed state. */
export function InstallGuide() {
  const standalone = useStandalone();
  const detected = useMemo(() => detectPlatform(), []);
  const guides = useMemo(() => [...GUIDES].sort((a, b) => Number(b.platform === detected) - Number(a.platform === detected)), [detected]);
  return (
    <SettingsLayout>
      <PageHeader title="Install Monoptah" description="Open Monoptah like any other app, full screen and one tap away." />
      {standalone ? (
        <Notice variant="info" title="Installed">
          You are using Monoptah as an installed app. You can turn on notifications below.
        </Notice>
      ) : (
        guides.map((guide) => <GuideCard key={guide.platform} guide={guide} current={guide.platform === detected} />)
      )}
      {standalone ? (
        <SettingsCard title="Notifications" aria-label="Notifications" footnote="Permission is asked only when you press the button.">
          <PushOnboarding />
        </SettingsCard>
      ) : null}
      {/* An installed app has no browser Back button: always offer a way into the app. */}
      <SettingsCard>
        <SettingsRow icon="computer" title="Back to Monoptah" to="/" />
      </SettingsCard>
    </SettingsLayout>
  );
}
