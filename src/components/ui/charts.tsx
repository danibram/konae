import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

/**
 * Charts of the design system. One rule runs through all of them: the colour
 * of a series follows its position in `--ds-s1` to `--ds-s5`, never its rank,
 * and every value a pointer can reach is also a focusable mark or a table row.
 */

const number = new Intl.NumberFormat("en");
export const formatNumber = (value: number) => number.format(value);
const compact = new Intl.NumberFormat("en", { notation: "compact", maximumFractionDigits: 1 });
const seriesColor = (index: number) => `var(--ds-s${(index % 5) + 1})`;

/** The smallest 1, 2 or 5 times a power of ten that is at least `value`. */
export function niceMax(value: number) {
  if (value <= 1) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  return [1, 2, 5, 10].map((step) => step * power).find((step) => step >= value) ?? 10 * power;
}

function ticksFor(max: number) {
  return [max, max / 2, 0].filter((tick, index, all) => Number.isInteger(tick) && all.indexOf(tick) === index);
}

export type ChartPoint = {
  key: string;
  /** What the tooltip and the axis call this point. */
  label: string;
  value: number;
  /** A breakdown of `value`. A bar stacks it; a tooltip lists it. */
  parts?: Record<string, number>;
};

type BarChartProps = {
  title: string;
  /** The unit a tooltip and the table name, as "page views". */
  unit: string;
  points: ChartPoint[];
  partLabel?: (part: string) => string;
  /** Stack each bar by its parts, one series colour per part in first-seen order. */
  stacked?: boolean;
  /** A fixed colour for a part whose meaning is a status, as a failure. The rest follow the series order. */
  partColor?: (part: string) => string | undefined;
  empty: string;
  /** Label of the first column of the table view. */
  keyHeading?: string;
};

/**
 * Counts as bars. Each column is focusable, its tooltip lists the parts, and
 * the same numbers sit in a table under the chart.
 */
export function BarChart({ title, unit, points, partLabel = (part) => part, stacked = false, partColor, empty, keyHeading = "Day" }: BarChartProps) {
  const [active, setActive] = useState<number | null>(null);
  const total = points.reduce((sum, point) => sum + point.value, 0);
  const max = niceMax(Math.max(0, ...points.map((point) => point.value)));
  const shown = active === null ? null : points[active];
  const ticks = ticksFor(max);
  const colorOf = (part: string, index: number) => partColor?.(part) ?? seriesColor(index);
  const partOrder = stacked ? [...new Set(points.flatMap((point) => Object.keys(point.parts ?? {})))].slice(0, 5) : [];
  return <figure className="ui-chart">
    <figcaption><span>{title}</span><strong>{formatNumber(total)}</strong></figcaption>
    {total === 0 ? <p className="ui-chart-empty">{empty}</p> : <>
      {stacked && partOrder.length > 1 && <ChartLegend items={partOrder.map((part, index) => ({ label: partLabel(part), color: colorOf(part, index) }))} />}
      <div className="ui-chart-plot" onPointerLeave={() => setActive(null)}>
        <div className="ui-chart-axis" aria-hidden="true">{ticks.map((tick) => <span key={tick} style={{ bottom: `${tick / max * 100}%` }}>{compact.format(tick)}</span>)}</div>
        <div className="ui-chart-bars" style={{ gridTemplateColumns: `repeat(${points.length}, minmax(0, 1fr))` }}>
          {ticks.map((tick) => <span key={tick} className="ui-chart-grid" data-baseline={tick === 0 || undefined} style={{ bottom: `${tick / max * 100}%` }} aria-hidden="true" />)}
          {points.map((point, index) => <button key={point.key} type="button" className="ui-chart-column" data-active={active === index || undefined}
            aria-label={`${point.label}, ${formatNumber(point.value)} ${unit}`}
            onPointerEnter={() => setActive(index)} onFocus={() => setActive(index)} onBlur={() => setActive(null)}>
            {point.value > 0 && (stacked && partOrder.length > 1
              ? <span className="ui-chart-stack" style={{ height: `${point.value / max * 100}%` }}>{partOrder.map((part, partIndex) => (point.parts?.[part] ?? 0) > 0 && <span key={part} style={{ flexGrow: point.parts![part], background: colorOf(part, partIndex) }} />)}</span>
              : <span className="ui-chart-bar" style={{ height: `${point.value / max * 100}%` }} />)}
          </button>)}
          {shown && <div className="ui-chart-tooltip" role="status" style={{ left: `${(active! + 0.5) / points.length * 100}%` }} data-edge={active! < points.length / 4 ? "start" : active! > points.length * 3 / 4 ? "end" : undefined}>
            <strong>{formatNumber(shown.value)}</strong> <span>{unit}</span>
            <small>{shown.label}</small>
            {shown.parts && Object.keys(shown.parts).length > 1 && <ul>{Object.entries(shown.parts).sort(([, a], [, b]) => b - a).map(([part, count]) => <li key={part}>{stacked && <i style={{ background: colorOf(part, partOrder.indexOf(part)) }} />}<strong>{formatNumber(count)}</strong> <span>{partLabel(part)}</span></li>)}</ul>}
          </div>}
        </div>
        <div className="ui-chart-x" aria-hidden="true"><span>{points[0].label}</span><span>{points[points.length - 1].label}</span></div>
      </div>
      <details className="ui-disclosure ui-chart-table"><summary>Show as a table</summary>
        <div className="ui-table-scroll"><table className="ui-table"><thead><tr><th scope="col">{keyHeading}</th><th scope="col">{unit[0].toUpperCase() + unit.slice(1)}</th><th scope="col">Detail</th></tr></thead>
          <tbody>{points.filter((point) => point.value > 0).map((point) => <tr key={point.key}><th scope="row">{point.label}</th><td>{formatNumber(point.value)}</td>
            <td>{Object.entries(point.parts ?? {}).map(([part, count]) => `${partLabel(part)} · ${formatNumber(count)}`).join("; ")}</td></tr>)}</tbody></table></div>
      </details>
    </>}
  </figure>;
}

export function ChartLegend({ items }: { items: { label: string; color: string; value?: string }[] }) {
  return <ul className="ui-legend">{items.map((item) => <li key={item.label}><i style={{ background: item.color }} aria-hidden="true" /><span>{item.label}</span>{item.value && <strong>{item.value}</strong>}</li>)}</ul>;
}

function useWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    setWidth(node.clientWidth);
    const observer = new ResizeObserver(([entry]) => setWidth(Math.round(entry.contentRect.width)));
    observer.observe(node);
    return () => observer.disconnect();
  }, []);
  return [ref, width] as const;
}

export type LineSeries = { id: string; label: string; values: number[] };

type LineChartProps = {
  title: string;
  unit: string;
  /** One label per x position, shared by every series. */
  labels: string[];
  series: LineSeries[];
  /** Fill the first series down to the baseline. */
  area?: boolean;
  height?: number;
  empty: string;
};

const MARGIN = { top: 10, right: 12, bottom: 24, left: 40 };

/**
 * One to five series over shared x positions. A crosshair follows the pointer
 * and the arrow keys; the plot is one tab stop and the values also sit in a table.
 */
export function LineChart({ title, unit, labels, series, area = false, height = 200, empty }: LineChartProps) {
  const [ref, width] = useWidth();
  const [active, setActive] = useState<number | null>(null);
  const shownSeries = series.slice(0, 5);
  const all = shownSeries.flatMap((entry) => entry.values);
  const max = niceMax(Math.max(0, ...all));
  const count = labels.length;
  const innerW = Math.max(0, width - MARGIN.left - MARGIN.right);
  const innerH = height - MARGIN.top - MARGIN.bottom;
  const x = (index: number) => MARGIN.left + (count <= 1 ? innerW / 2 : index / (count - 1) * innerW);
  const y = (value: number) => MARGIN.top + innerH - value / max * innerH;
  const ticks = ticksFor(max);
  const total = all.reduce((sum, value) => sum + value, 0);
  const path = (values: number[]) => values.map((value, index) => `${index === 0 ? "M" : "L"}${x(index).toFixed(1)} ${y(value).toFixed(1)}`).join(" ");
  const move = (event: { clientX: number; currentTarget: Element }) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const ratio = (event.clientX - rect.left - MARGIN.left) / Math.max(1, innerW);
    setActive(Math.min(count - 1, Math.max(0, Math.round(ratio * (count - 1)))));
  };
  const key = (event: KeyboardEvent) => {
    if (event.key === "ArrowRight") setActive((current) => Math.min(count - 1, (current ?? -1) + 1));
    else if (event.key === "ArrowLeft") setActive((current) => Math.max(0, (current ?? count) - 1));
    else if (event.key === "Escape") setActive(null);
    else return;
    event.preventDefault();
  };
  const tooltipLeft = active === null ? 0 : x(active);
  return <figure className="ui-chart">
    <figcaption><span>{title}</span><strong>{formatNumber(shownSeries.reduce((sum, entry) => sum + entry.values.reduce((a, b) => a + b, 0), 0))}</strong></figcaption>
    {total === 0 ? <p className="ui-chart-empty">{empty}</p> : <>
      {shownSeries.length > 1 && <ChartLegend items={shownSeries.map((entry, index) => ({ label: entry.label, color: seriesColor(index) }))} />}
      <div className="ui-line" ref={ref} style={{ height }}>
        {width > 0 && <svg width={width} height={height} role="group" tabIndex={0} aria-label={`${title}. Use the left and right arrow keys to read each point.`}
          onPointerMove={move} onPointerLeave={() => setActive(null)} onBlur={() => setActive(null)} onKeyDown={key}>
          {ticks.map((tick) => <g key={tick}>
            <line x1={MARGIN.left} x2={width - MARGIN.right} y1={y(tick)} y2={y(tick)} className={tick === 0 ? "ui-line-base" : "ui-line-grid"} />
            <text x={MARGIN.left - 8} y={y(tick)} dy="0.32em" textAnchor="end" className="ui-line-tick">{compact.format(tick)}</text>
          </g>)}
          {area && shownSeries[0] && <path d={`${path(shownSeries[0].values)} L${x(count - 1).toFixed(1)} ${y(0)} L${x(0).toFixed(1)} ${y(0)} Z`} className="ui-line-area" style={{ fill: seriesColor(0) }} />}
          {shownSeries.map((entry, index) => <path key={entry.id} d={path(entry.values)} className="ui-line-path" style={{ stroke: seriesColor(index) }} />)}
          <text x={MARGIN.left} y={height - 6} className="ui-line-tick">{labels[0]}</text>
          <text x={width - MARGIN.right} y={height - 6} textAnchor="end" className="ui-line-tick">{labels[count - 1]}</text>
          {active !== null && <g>
            <line x1={x(active)} x2={x(active)} y1={MARGIN.top} y2={MARGIN.top + innerH} className="ui-line-cross" />
            {shownSeries.map((entry, index) => <circle key={entry.id} cx={x(active)} cy={y(entry.values[active])} r={5} className="ui-line-dot" style={{ fill: seriesColor(index) }} />)}
          </g>}
        </svg>}
        {active !== null && <div className="ui-chart-tooltip" role="status" style={{ left: tooltipLeft, top: 0 }} data-edge={tooltipLeft < width / 4 ? "start" : tooltipLeft > width * 3 / 4 ? "end" : undefined}>
          <small>{labels[active]}</small>
          <ul>{shownSeries.map((entry, index) => <li key={entry.id}><i style={{ background: seriesColor(index) }} /><strong>{formatNumber(entry.values[active])}</strong> <span>{entry.label}</span></li>)}</ul>
        </div>}
      </div>
      <details className="ui-disclosure ui-chart-table"><summary>Show as a table</summary>
        <div className="ui-table-scroll"><table className="ui-table"><thead><tr><th scope="col" />{shownSeries.map((entry) => <th scope="col" key={entry.id}>{entry.label}</th>)}</tr></thead>
          <tbody>{labels.map((label, index) => <tr key={label + index}><th scope="row">{label}</th>{shownSeries.map((entry) => <td key={entry.id}>{formatNumber(entry.values[index])}</td>)}</tr>)}</tbody></table></div>
      </details>
    </>}
  </figure>;
}

export type Slice = { id: string; label: string; value: number };

/** Shares of one whole, at most five slices; a sixth belongs in "Other" before it gets here. */
export function DonutChart({ title, unit, slices, empty, center }: { title: string; unit: string; slices: Slice[]; empty: string; center?: ReactNode }) {
  const [active, setActive] = useState<number | null>(null);
  const shown = slices.slice(0, 5);
  const total = shown.reduce((sum, slice) => sum + slice.value, 0);
  const radius = 54;
  const circumference = 2 * Math.PI * radius;
  let offset = 0;
  const gap = shown.length > 1 ? 3 : 0;
  return <figure className="ui-chart ui-donut">
    <figcaption><span>{title}</span></figcaption>
    {total === 0 ? <p className="ui-chart-empty">{empty}</p> : <div className="ui-donut-body">
      <svg viewBox="0 0 140 140" role="img" aria-label={`${title}: ${shown.map((slice) => `${slice.label} ${formatNumber(slice.value)} ${unit}`).join(", ")}`} onPointerLeave={() => setActive(null)}>
        <circle cx="70" cy="70" r={radius} className="ui-donut-track" />
        {shown.map((slice, index) => {
          const length = slice.value / total * circumference;
          const dash = Math.max(0, length - gap);
          const node = <circle key={slice.id} cx="70" cy="70" r={radius} className="ui-donut-slice" data-dim={active !== null && active !== index || undefined} style={{ stroke: seriesColor(index) }}
            strokeDasharray={`${dash} ${circumference - dash}`} strokeDashoffset={-offset} transform="rotate(-90 70 70)" onPointerEnter={() => setActive(index)} />;
          offset += length;
          return node;
        })}
        <text x="70" y="66" textAnchor="middle" className="ui-donut-value">{formatNumber(active === null ? total : shown[active].value)}</text>
        <text x="70" y="84" textAnchor="middle" className="ui-donut-unit">{active === null ? unit : shown[active].label}</text>
      </svg>
      <div>
        <ul className="ui-legend ui-legend-list">{shown.map((slice, index) => <li key={slice.id} data-dim={active !== null && active !== index || undefined} onPointerEnter={() => setActive(index)} onPointerLeave={() => setActive(null)}>
          <i style={{ background: seriesColor(index) }} aria-hidden="true" /><span>{slice.label}</span><strong>{formatNumber(slice.value)}</strong><small>{Math.round(slice.value / total * 100)}%</small></li>)}</ul>
        {center}
      </div>
    </div>}
  </figure>;
}

/** A trend with no axes, for a stat tile. The tile's text carries the number. */
export function Sparkline({ values, label }: { values: number[]; label: string }) {
  const width = 96;
  const height = 28;
  const max = Math.max(1, ...values);
  const step = values.length > 1 ? (width - 4) / (values.length - 1) : 0;
  const point = (value: number, index: number) => `${(2 + index * step).toFixed(1)} ${(height - 3 - value / max * (height - 6)).toFixed(1)}`;
  const line = values.map((value, index) => `${index === 0 ? "M" : "L"}${point(value, index)}`).join(" ");
  const last = values.length - 1;
  const [lx, ly] = point(values[last] ?? 0, Math.max(0, last)).split(" ");
  return <svg className="ui-spark" width={width} height={height} viewBox={`0 0 ${width} ${height}`} role="img" aria-label={label}>
    <path d={line} />{values.length > 0 && <circle cx={lx} cy={ly} r="2.5" />}
  </svg>;
}

export type BarRow = {
  key: string;
  label: ReactNode;
  value: number;
  /** Words after the figure, as a share of a quota. */
  detail?: ReactNode;
  /** A row past a limit the reader must notice. The detail says why in words. */
  tone?: "warning" | "danger";
};

/**
 * Ranked horizontal bars on one baseline, largest first, one hue. Every row
 * prints its figure, so nothing hides behind a hover, and the bars compare
 * parts more exactly than a donut's slices.
 */
export function BarList({ title, rows, format = formatNumber, total, empty, labelHeading = "Name", unit }: {
  title: string;
  rows: BarRow[];
  format?: (value: number) => string;
  /** The figure the caption shows. The sum of the rows when absent. */
  total?: number;
  empty: string;
  labelHeading?: string;
  /** What a value counts, for screen readers, as "actions". */
  unit: string;
}) {
  const sorted = [...rows].sort((a, b) => b.value - a.value);
  const max = Math.max(0, ...sorted.map((row) => row.value));
  const sum = total ?? sorted.reduce((acc, row) => acc + row.value, 0);
  return <figure className="ui-chart ui-barlist">
    <figcaption><span>{title}</span><strong>{format(sum)}</strong></figcaption>
    {max === 0 ? <p className="ui-chart-empty">{empty}</p> : <table className="ui-barlist-rows">
      <thead><tr><th scope="col">{labelHeading}</th><td /><th scope="col">{unit[0].toUpperCase() + unit.slice(1)}</th></tr></thead>
      <tbody>{sorted.map((row) => <tr key={row.key} data-tone={row.tone}>
        <th scope="row">{row.label}{row.detail && <small>{row.detail}</small>}</th>
        <td className="ui-barlist-bar" aria-hidden="true">{row.value > 0 && <span style={{ width: `${row.value / max * 100}%` }} />}</td>
        <td className="ui-barlist-value">{format(row.value)}</td>
      </tr>)}</tbody>
    </table>}
  </figure>;
}
