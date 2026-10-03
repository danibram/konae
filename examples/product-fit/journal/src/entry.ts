export type Entry = { id: number; body: string; mood: string; created_at: number };
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]!));
export const renderEntry = (e: Entry) =>
  `<li><time>${new Date(e.created_at).toLocaleString()} · ${esc(e.mood)}</time><p>${esc(e.body)}</p><button data-id="${e.id}" aria-label="Delete the entry from ${new Date(e.created_at).toLocaleString()}">delete</button></li>`;
