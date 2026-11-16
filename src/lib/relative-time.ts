const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const dated = new Intl.DateTimeFormat("en", { month: "short", day: "numeric" });
const datedWithYear = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", year: "numeric" });

export function relativeTime(at: number, now = Date.now()): string {
  const age = now - at;
  if (age < MINUTE) return "just now";
  if (age < HOUR) return `${Math.floor(age / MINUTE)} min ago`;
  if (age < DAY) return `${Math.floor(age / HOUR)} h ago`;
  if (age < 2 * DAY) return "yesterday";
  if (age < 7 * DAY) return `${Math.floor(age / DAY)} days ago`;
  return new Date(at).getFullYear() === new Date(now).getFullYear() ? dated.format(at) : datedWithYear.format(at);
}

export function exactTime(at: number): string {
  return new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" }).format(at);
}
