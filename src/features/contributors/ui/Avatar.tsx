import { colorForPerson } from "../model/palette";

const SIZES = {
  sm: "size-6 text-[10px]",
  md: "size-7 text-[11px]",
  lg: "size-11 text-[16px]",
} as const;

/** "Ana Bui" -> "AB", "ana" -> "A", "" -> "?". */
export function initialsOf(name: string): string {
  const words = name
    .replace(/@.*$/, "")
    .split(/[\s._-]+/)
    .filter(Boolean);
  if (words.length === 0) return "?";
  const first = Array.from(words[0])[0] ?? "";
  const last =
    words.length > 1 ? (Array.from(words[words.length - 1])[0] ?? "") : "";
  return (first + last).toUpperCase() || "?";
}

export function Avatar({
  name,
  color,
  personId,
  size = "sm",
}: {
  name: string;
  /** Wins over the id-derived color. */
  color?: string;
  /** Gives the person the same color everywhere when no color is passed. */
  personId?: string;
  size?: keyof typeof SIZES;
}) {
  return (
    <span
      aria-hidden
      className={`grid shrink-0 place-items-center rounded-full font-semibold text-white ${SIZES[size]}`}
      style={{ backgroundColor: color ?? colorForPerson(personId ?? name) }}
    >
      {initialsOf(name)}
    </span>
  );
}
