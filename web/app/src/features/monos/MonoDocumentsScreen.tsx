// F-76: the documents list page `/d/:desktopId/monos/:monoId/profile/documents`: everything the Mono wrote, newest
// first (mobile MonoDocuments.tsx). Read-only.
import { Link } from "@tanstack/react-router";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { MonoDocumentRows } from "./MonoDocumentRows";
import { BACK_LINK_CLASS, BackIcon, Group, MonoPage, RefreshButton } from "./MonoPage";
import { useMonoProfile } from "./profile";

export function MonoDocumentsScreen({ desktopId, monoId }: { desktopId: string; monoId: string }) {
  const { profile, loading, error, refreshing, refresh } = useMonoProfile(monoId);
  return (
    <MonoPage
      testId="mono-documents"
      title="Documents"
      back={
        <Link to="/d/$desktopId/monos/$monoId/profile" params={{ desktopId, monoId }} aria-label="Back" className={BACK_LINK_CLASS}>
          <BackIcon />
        </Link>
      }
      actions={<RefreshButton onRefresh={refresh} refreshing={refreshing} />}
    >
      {profile ? (
        <Group title={`${profile.documents.length} ${profile.documents.length === 1 ? "document" : "documents"}`}>
          <MonoDocumentRows desktopId={desktopId} monoId={monoId} documents={profile.documents} />
        </Group>
      ) : error ? (
        <Notice variant="error">{error}</Notice>
      ) : loading ? (
        <DelayedSpinner label="Loading the documents" className="m-6" />
      ) : null}
    </MonoPage>
  );
}
