// Copied from src/features/sessions/ui/AttachmentChip.tsx at ca73ac2b
// Without ImageLightbox: a photo chip calls `onPreview` and the caller decides how to show it
// (a shadcn Dialog). The file-type icon is the generic file glyph until FileTypeIcon is ported.
import { cn } from "@/lib/utils";
import { File, X } from "@/ui/icons";
import { isImage, type ComposerAttachment } from "./attachments";

export interface AttachmentChipProps {
  attachment: Pick<ComposerAttachment, "name" | "mimeType" | "previewUrl">;
  onRemove?: () => void;
  /** Opens a larger view of an image attachment. Without it the thumbnail is not a button. */
  onPreview?: () => void;
  /** A sent image shown large on its own, like a photo in a messaging app. */
  photo?: boolean;
  className?: string;
}

export function AttachmentChip({ attachment, onRemove, onPreview, photo = false, className }: AttachmentChipProps) {
  const preview = isImage(attachment) ? attachment.previewUrl : undefined;
  const thumbnail = preview ? (
    <img
      src={preview}
      alt=""
      draggable={false}
      className={cn("block object-cover", photo ? "max-h-60 max-w-60 rounded-2xl" : "size-11 rounded-row")}
    />
  ) : null;

  return (
    <div
      className={cn(
        "group relative flex min-w-0 items-center gap-1.5 rounded-row",
        thumbnail ? "" : "bg-fill py-1 pl-2 pr-1",
        className,
      )}
      title={attachment.name}
      data-attachment-chip
    >
      {thumbnail ? (
        onPreview ? (
          <button
            type="button"
            aria-label={`Open ${attachment.name}`}
            title={`Open ${attachment.name}`}
            onClick={(event) => {
              event.stopPropagation();
              onPreview();
            }}
            className={cn("cursor-zoom-in focus-visible:outline-2 focus-visible:outline-brand", photo ? "min-w-0 rounded-2xl" : "shrink-0 rounded-row")}
          >
            {thumbnail}
          </button>
        ) : (
          thumbnail
        )
      ) : (
        <>
          <File size={16} className="shrink-0 text-muted-text" aria-hidden="true" />
          <span className="min-w-0 max-w-36 truncate text-caption text-content">{attachment.name}</span>
        </>
      )}
      {onRemove ? (
        <button
          type="button"
          title="Remove"
          aria-label={`Remove ${attachment.name}`}
          onClick={(event) => {
            event.stopPropagation();
            onRemove();
          }}
          className={cn(
            "grid shrink-0 place-items-center rounded-full text-muted-text hover:text-content focus-visible:outline-2 focus-visible:outline-brand",
            thumbnail ? "absolute -right-1 -top-1 size-6 bg-fill" : "size-6",
          )}
        >
          <X size={12} strokeWidth={2} />
        </button>
      ) : null}
    </div>
  );
}
