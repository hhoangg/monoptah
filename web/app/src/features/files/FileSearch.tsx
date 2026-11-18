// F-61: the search box of the tree. The query is sent to `project.search` after 150 ms without typing.
import { useLinkQuery } from "@/data";
import { scorePath } from "@/lib/fuzzy";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { MatchText } from "@/ui/MatchText";
import { FileTypeIcon } from "./FileTypeIcon";
import { SEARCH_DEBOUNCE_MS, useDebounced } from "./filePaths";

export interface SearchHit {
  path: string;
  name: string;
  positions: number[];
}

/** Desktop hits ranked by fuzzy score (filename hits first), with the matched positions of the path. */
export function rankHits(query: string, results: ReadonlyArray<{ path: string; name: string }>): SearchHit[] {
  const ranked = results.map((result, index) => {
    const hit = scorePath(query, result.path, result.name);
    return { result, index, score: hit?.score ?? -1, positions: hit?.positions ?? [] };
  });
  ranked.sort((a, b) => b.score - a.score || a.index - b.index);
  return ranked.map(({ result, positions }) => ({ path: result.path, name: result.name, positions }));
}

/** `project.search` for an already debounced `query` ("" sends nothing). */
export function useFileSearchResults(projectId: string, query: string) {
  const result = useLinkQuery("project.search", { projectId, query }, { enabled: query.length > 0 });
  const hits = result.data && query ? rankHits(query, result.data.results) : [];
  return { hits, isPending: query.length > 0 && result.isPending, isError: result.isError };
}

/** `project.search` for `text`, sent 150 ms after the last keystroke. `query` is what was sent. */
export function useFileSearch(projectId: string, text: string) {
  const query = useDebounced(text.trim(), SEARCH_DEBOUNCE_MS);
  return { query, ...useFileSearchResults(projectId, query) };
}

export function SearchBox({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  return (
    <div className="relative px-3">
      <Icon name="search" size={16} className="pointer-events-none absolute top-1/2 left-6 -translate-y-1/2 text-muted-text" />
      <Input
        type="search"
        role="searchbox"
        aria-label="Search files"
        placeholder="Search files"
        autoComplete="off"
        spellCheck={false}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        className="pl-9"
      />
    </div>
  );
}

/** `query` is the debounced text: the debounce lives in the pane, which stays mounted while typing starts. */
export function SearchResults({ projectId, query, onOpenFile }: { projectId: string; query: string; onOpenFile: (path: string) => void }) {
  const { hits, isPending, isError } = useFileSearchResults(projectId, query);
  if (isError) return <p className="px-4 py-3 text-small text-deletions">The search failed.</p>;
  if (isPending || !query) return <DelayedSpinner label="Searching" className="m-4" />;
  if (hits.length === 0) return <p className="px-4 py-3 text-small text-muted-text">No files match "{query}".</p>;
  return (
    <ul aria-label="Search results" className="px-1 pb-3">
      {hits.map((hit) => (
        <li key={hit.path}>
          <button
            type="button"
            onClick={() => onOpenFile(hit.path)}
            className="flex min-h-11 w-full items-center gap-2 rounded-row px-2 text-left text-label hover:bg-selection pointer-fine:min-h-8"
          >
            <FileTypeIcon name={hit.name} isDir={false} />
            <span className="min-w-0 flex-1 truncate [unicode-bidi:isolate]">
              <MatchText text={hit.path} positions={hit.positions} active />
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
