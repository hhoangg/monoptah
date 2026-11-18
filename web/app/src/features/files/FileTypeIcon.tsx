// Copied from src/features/files/ui/FileTypeIcon.tsx at c579af2e
// (the Material icon pack and its raw SVG markup injection are replaced
// by hugeicons through @/ui/Icon; the name -> kind logic lives in @/lib/fileKind).
/**
 * `<FileTypeIcon name isDir isOpen? size? />` renders a decorative (aria-hidden)
 * icon for a file or folder entry. `name` may be a leaf name or a path.
 * `iconForKind(kind)` exposes the kind -> icon mapping for other screens.
 */
import { memo } from "react";
import { fileKind, type FileKind } from "@/lib/fileKind";
import { Icon, type IconName } from "@/ui/Icon";

const ICON_BY_KIND: Record<FileKind, IconName> = {
  code: "source",
  markup: "source",
  style: "source",
  data: "list",
  config: "settings",
  markdown: "note",
  text: "note",
  image: "image",
  media: "image",
  archive: "archive",
  shell: "terminal",
  lock: "lock",
  secret: "key",
  binary: "file",
  file: "file",
};

export function iconForKind(kind: FileKind): IconName {
  return ICON_BY_KIND[kind];
}

type Props = {
  name: string;
  isDir: boolean;
  isOpen?: boolean;
  size?: number;
  className?: string;
};

export const FileTypeIcon = memo(function FileTypeIcon({ name, isDir, isOpen = false, size = 16, className }: Props) {
  const icon: IconName = isDir ? "folder" : iconForKind(fileKind(name));
  return (
    <span
      aria-hidden
      data-kind={isDir ? (isOpen ? "folder-open" : "folder") : fileKind(name)}
      className={`inline-flex shrink-0 items-center justify-center align-middle ${isDir ? "text-brand" : "text-muted-text"} ${className ?? ""}`}
      style={{ width: size, height: size }}
    >
      <Icon name={icon} size={size} />
    </span>
  );
});
