export function safeReturnPath(raw: string | null, origin: string = window.location.origin): string | null {
  if (!raw || !raw.startsWith("/")) return null;
  let url: URL;
  try { url = new URL(raw, origin); } catch { return null; }
  if (url.origin !== origin) return null;
  return url.pathname + url.search + url.hash;
}
