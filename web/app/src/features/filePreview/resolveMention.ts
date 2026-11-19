// Where a file mention opens: a text or source file inside the session's own project goes to the project's Files
// route (the existing `project.file` method, with a line jump); everything else opens the preview.
import { fileKind, leafName } from "@/lib/fileKind";
import { pathKey, slash } from "@/lib/paths";

export type MentionRoute = { to: "project"; path: string } | { to: "preview" };

/** Kinds the Files route shows well as text. Markdown, html, svg and images have a richer preview. */
const FILES_ROUTE_KINDS = new Set(["code", "markup", "style", "data", "config", "text", "shell", "lock"]);

function opensInFiles(path: string): boolean {
  const name = leafName(path).toLowerCase();
  if (/\.(html?|svg)$/.test(name)) return false;
  return FILES_ROUTE_KINDS.has(fileKind(name));
}

/** `path` relative to `projectPath` when it is inside it, else null. */
function insideProject(path: string, projectPath: string): string | null {
  const root = slash(projectPath).replace(/\/+$/, "");
  const target = slash(path);
  if (!root || root === "~") return null;
  if (target.startsWith("/") || /^[a-z]:\//i.test(target)) {
    const rootKey = pathKey(root);
    const key = pathKey(target);
    return key.startsWith(`${rootKey}/`) ? target.slice(root.length + 1) : null;
  }
  if (target.startsWith("~")) return null;
  const relative = target.split("/").filter((part) => part !== "." && part !== "").join("/");
  return relative && !relative.split("/").includes("..") ? relative : null;
}

export function routeMention(path: string, project: { path: string } | undefined): MentionRoute {
  if (!project) return { to: "preview" };
  const relative = insideProject(path, project.path);
  return relative !== null && opensInFiles(relative) ? { to: "project", path: relative } : { to: "preview" };
}
