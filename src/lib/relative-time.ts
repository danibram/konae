const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const dated = new Intl.DateTimeFormat("en", { month: "short", day: "numeric" });
const datedWithYear = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", year: "numeric" });

/**
 * How long ago a moment was, in the words a person uses: "just now", "3 min ago",
 * "2 h ago", "yesterday", "4 days ago", then the date. A moment in the future reads as "just now".
 */
export function relativeTime(at: number, now = Date.now()): string {
  const age = now - at;
  if (age < MINUTE) return "just now";
  if (age < HOUR) return `${Math.floor(age / MINUTE)} min ago`;
  if (age < DAY) return `${Math.floor(age / HOUR)} h ago`;
  if (age < 2 * DAY) return "yesterday";
  if (age < 7 * DAY) return `${Math.floor(age / DAY)} days ago`;
  return new Date(at).getFullYear() === new Date(now).getFullYear() ? dated.format(at) : datedWithYear.format(at);
}

/** The full date and time, for a title attribute beside a relative time. */
export function exactTime(at: number): string {
  return new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" }).format(at);
}
