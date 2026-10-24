import type { DailyPoint } from "../lib/usage";
import { BarChart } from "./ui";

const dayLabel = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" });
const formatDay = (day: string) => dayLabel.format(Date.parse(`${day}T00:00:00Z`));

type Props = {
  title: string;
  /** The unit a tooltip and the table name, as "page views". */
  unit: string;
  points: DailyPoint[];
  /** Words for one part of a day, as a page's file or an outcome's label. */
  partLabel?: (part: string) => string;
  empty: string;
  stacked?: boolean;
  partColor?: (part: string) => string | undefined;
};

/** Usage's daily counts as the design system's bar chart. */
export function DailyChart({ points, ...props }: Props) {
  return <BarChart {...props} keyHeading="Day (UTC)" points={points.map((point) => ({ key: point.day, label: formatDay(point.day), value: point.value, parts: point.parts }))} />;
}
