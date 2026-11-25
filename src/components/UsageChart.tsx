import type { DailyPoint } from "../lib/usage";
import { BarChart } from "./ui";

const dayLabel = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" });
const formatDay = (day: string) => dayLabel.format(Date.parse(`${day}T00:00:00Z`));

type Props = {
  title: string;
  unit: string;
  points: DailyPoint[];
  partLabel?: (part: string) => string;
  empty: string;
  stacked?: boolean;
  showSinglePart?: boolean;
  partColor?: (part: string) => string | undefined;
};

export function DailyChart({ points, ...props }: Props) {
  return <BarChart {...props} keyHeading="Day (UTC)" points={points.map((point) => ({ key: point.day, label: formatDay(point.day), value: point.value, parts: point.parts }))} />;
}
