/**
 * The id GitHub gives a heading, so one `#anchor` works in the repository and
 * on the site. `seen` counts repeats within a page: the second "Example"
 * becomes `example-1`.
 */
export function slug(text, seen = new Map()) {
  const base = text.trim().toLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, "").replace(/\s/g, "-");
  const count = seen.get(base) ?? 0;
  seen.set(base, count + 1);
  return count ? `${base}-${count}` : base;
}
