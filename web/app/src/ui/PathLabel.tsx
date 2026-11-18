// A project-relative path where the file name is read first: the name is never the part that gets cut, the folder
// (muted) comes after it and truncates. The full path is the tooltip. `positions` accents fuzzy-match characters of `path`.
import { cn } from "@/lib/utils";
import { MatchText } from "./MatchText";

export interface PathLabelProps {
  path: string;
  /** Characters of `path` to accent (search hits). */
  positions?: readonly number[];
  className?: string;
}

export function PathLabel({ path, positions = [], className }: PathLabelProps) {
  const cut = path.lastIndexOf("/");
  const name = path.slice(cut + 1);
  const dir = cut > 0 ? path.slice(0, cut) : "";
  const inName = positions.filter((position) => position > cut).map((position) => position - cut - 1);
  const inDir = positions.filter((position) => position < cut);
  return (
    <span title={path} className={cn("flex min-w-0 items-baseline gap-2 [unicode-bidi:isolate]", className)}>
      <span className="max-w-full min-w-0 shrink-0 truncate font-medium">
        <MatchText text={name} positions={inName} active={inName.length > 0} />
      </span>
      {dir ? (
        <span className="min-w-0 truncate text-small font-normal text-muted-text">
          <MatchText text={dir} positions={inDir} active={inDir.length > 0} />
        </span>
      ) : null}
    </span>
  );
}
