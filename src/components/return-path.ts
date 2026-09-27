/**
 * Accepts a same-origin path only, so a return link never sends the owner to
 * another site. The URL parser decides, because it removes tabs and newlines
 * and reads a backslash as a slash before it resolves the path: `/\t/evil.example`
 * is `//evil.example` to the browser, and a prefix check on the raw text does
 * not see that.
 */
export function safeReturnPath(raw: string | null, origin: string = window.location.origin): string | null {
  if (!raw || !raw.startsWith("/")) return null;
  let url: URL;
  try { url = new URL(raw, origin); } catch { return null; }
  if (url.origin !== origin) return null;
  return url.pathname + url.search + url.hash;
}
