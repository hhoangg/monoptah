// Whether a path is a file or a folder: the listing of its parent says. A path the listing does not
// show (a hidden or missing file) counts as a file, so the viewer reports why it cannot be opened.
import { useLinkQuery } from "@/data";
import { folderParams, nameOf, parentPath } from "./filePaths";

export type EntryKind = "root" | "folder" | "file" | "loading";

export function useEntryKind(projectId: string, path: string): EntryKind {
  const parent = useLinkQuery("project.files", folderParams(projectId, parentPath(path)), { enabled: path !== "" });
  if (path === "") return "root";
  if (parent.isError) return "file";
  if (!parent.data) return "loading";
  const entry = parent.data.entries.find((candidate) => candidate.name === nameOf(path));
  return entry?.isDir ? "folder" : "file";
}
