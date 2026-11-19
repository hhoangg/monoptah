/** `path` without its trailing slashes, in linear time (a regex anchored at the end is quadratic on a long run). */
export function trimSlashes(path: string): string {
  let end = path.length;
  while (end > 0 && path.charCodeAt(end - 1) === 47) end -= 1;
  return end === path.length ? path : path.slice(0, end);
}
