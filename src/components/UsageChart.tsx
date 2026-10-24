import { useState } from "react";
import type { DailyPoint } from "../lib/usage";

const number = new Intl.NumberFormat("en");
const dayLabel = new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" });
const formatDay = (day: string) => dayLabel.format(Date.parse(`${day}T00:00:00Z`));

/** The smallest 1, 2 or 5 times a power of ten that is at least `value`. */
export function niceMax(value: number) {
  if (value <= 1) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  return [1, 2, 5, 10].map((step) => step * power).find((step) => step >= value) ?? 10 * power;
}

type Props = {
  title: string;
  /** The unit a tooltip and the table name, as "page views". */
  unit: string;
  points: DailyPoint[];
  /** Words for one part of a day, as a page's file or an outcome's label. */
  partLabel?: (part: string) => string;
  empty: string;
};

/**
 * One series of daily counts as bars over the selected period. Each day is a
 * focusable column whose tooltip lists the day's parts, and the same numbers
 * sit in a table under the chart, so no value needs a pointer to be read.
 */
export function DailyChart({ title, unit, points, partLabel = (part) => part, empty }: Props) {
  const [active, setActive] = useState<number | null>(null);
  const total = points.reduce((sum, point) => sum + point.value, 0);
  const max = niceMax(Math.max(0, ...points.map((point) => point.value)));
  const shown = active === null ? null : points[active];
  const ticks = [max, max / 2, 0].filter((tick, index, all) => Number.isInteger(tick) && all.indexOf(tick) === index);
  return <figure className="usage-chart">
    <figcaption><span>{title}</span><strong>{number.format(total)}</strong></figcaption>
    {total === 0 ? <p className="usage-note">{empty}</p> : <>
      <div className="usage-chart-plot" onPointerLeave={() => setActive(null)}>
        <div className="usage-chart-axis" aria-hidden="true">{ticks.map((tick) => <span key={tick} style={{ bottom: `${tick / max * 100}%` }}>{number.format(tick)}</span>)}</div>
        <div className="usage-chart-bars" style={{ gridTemplateColumns: `repeat(${points.length}, minmax(0, 1fr))` }}>
          {ticks.map((tick) => <span key={tick} className="usage-chart-grid" data-baseline={tick === 0 || undefined} style={{ bottom: `${tick / max * 100}%` }} aria-hidden="true" />)}
          {points.map((point, index) => <button key={point.day} type="button" className="usage-chart-column" data-active={active === index || undefined}
            aria-label={`${formatDay(point.day)}, ${number.format(point.value)} ${unit}`}
            onPointerEnter={() => setActive(index)} onFocus={() => setActive(index)} onBlur={() => setActive(null)}>
            {point.value > 0 && <span className="usage-chart-bar" style={{ height: `${point.value / max * 100}%` }} />}
          </button>)}
          {shown && <div className="usage-chart-tooltip" role="status" style={{ left: `${(active! + 0.5) / points.length * 100}%` }} data-edge={active! < points.length / 4 ? "start" : active! > points.length * 3 / 4 ? "end" : undefined}>
            <strong>{number.format(shown.value)}</strong> <span>{unit}</span>
            <small>{formatDay(shown.day)}</small>
            {Object.keys(shown.parts).length > 1 && <ul>{Object.entries(shown.parts).sort(([, a], [, b]) => b - a).map(([part, count]) => <li key={part}><strong>{number.format(count)}</strong> <span>{partLabel(part)}</span></li>)}</ul>}
          </div>}
        </div>
        <div className="usage-chart-x" aria-hidden="true"><span>{formatDay(points[0].day)}</span><span>{formatDay(points[points.length - 1].day)}</span></div>
      </div>
      <details className="usage-chart-table"><summary>Show as a table</summary>
        <table className="usage-table"><thead><tr><th scope="col">Day (UTC)</th><th scope="col">{unit[0].toUpperCase() + unit.slice(1)}</th><th scope="col">Detail</th></tr></thead>
          <tbody>{points.filter((point) => point.value > 0).map((point) => <tr key={point.day}><th scope="row">{point.day}</th><td>{number.format(point.value)}</td>
            <td>{Object.entries(point.parts).map(([part, count]) => `${partLabel(part)} · ${number.format(count)}`).join("; ")}</td></tr>)}</tbody></table>
      </details>
    </>}
  </figure>;
}
