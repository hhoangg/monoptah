// The tree column: the search box over the tree, replaced by the search results while a query is typed.
import type { ReactNode } from "react";
import { SearchBox, SearchResults } from "./FileSearch";
import { SEARCH_DEBOUNCE_MS, useDebounced } from "./filePaths";

export interface FileSearchPaneProps {
  projectId: string;
  search: string;
  onSearch: (value: string) => void;
  onOpenFile: (path: string) => void;
  children: ReactNode;
}

export function FileSearchPane({ projectId, search, onSearch, onOpenFile, children }: FileSearchPaneProps) {
  const query = useDebounced(search.trim(), SEARCH_DEBOUNCE_MS);
  return (
    <div className="flex h-full min-h-0 flex-col gap-2 pt-3">
      <SearchBox value={search} onChange={onSearch} />
      {search.trim() ? (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <SearchResults projectId={projectId} query={query} onOpenFile={onOpenFile} />
        </div>
      ) : (
        <div className="min-h-0 flex-1">{children}</div>
      )}
    </div>
  );
}
