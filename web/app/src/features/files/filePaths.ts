// Path helpers of the Files screens. Paths are project-relative with "/" separators, "" is the root.
import { useEffect, useState } from "react";
import { leafName } from "@/lib/fileKind";

export const HIDDEN_FILE_MESSAGE = "This file is hidden by Monoptah";
export const SEARCH_DEBOUNCE_MS = 150;

/** The route splat as a clean relative path ("" for the root). */
export function cleanPath(splat: string | undefined): string {
  return (splat ?? "").split("/").filter(Boolean).join("/");
}

export function parentPath(path: string): string {
  const index = path.lastIndexOf("/");
  return index < 0 ? "" : path.slice(0, index);
}

/** "a/b/c" -> ["a", "a/b", "a/b/c"]. */
export function ancestorsOf(path: string): string[] {
  const parts = path.split("/").filter(Boolean);
  return parts.map((_, index) => parts.slice(0, index + 1).join("/"));
}

export function nameOf(path: string): string {
  return leafName(path);
}

/** Params of `project.files`: the root has no `path`. */
export function folderParams(projectId: string, path: string) {
  return path ? { projectId, path } : { projectId };
}

export type FileLoadProblem = "denied" | "failed";

export function problemOf(error: unknown): FileLoadProblem {
  const code = typeof error === "object" && error !== null && "code" in error ? (error as { code?: unknown }).code : undefined;
  return code === "path_denied" ? "denied" : "failed";
}

/** `value` after it stayed unchanged for `delay` ms. */
export function useDebounced<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    if (Object.is(debounced, value)) return;
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay, debounced]);
  return debounced;
}
