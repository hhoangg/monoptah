/** Current time in unix seconds, the unit of every D1 time column. */
export function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}
