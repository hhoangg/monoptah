// Copied from src/shared/lib/numbers.ts at ca73ac2b
const INTEGER_FORMATTER = new Intl.NumberFormat("en-US", {
  maximumFractionDigits: 0,
});

/** Format whole-number UI counts with stable thousands separators. */
export function formatInteger(value: number): string {
  return INTEGER_FORMATTER.format(value);
}
