import type { FunctionLatencySummary } from "../lib/usage";

const milliseconds = new Intl.NumberFormat("en", { maximumFractionDigits: 2 });
const count = new Intl.NumberFormat("en");
const ms = (value: number) => `${milliseconds.format(value)} ms`;

export function FunctionLatency({ summary }: { summary: FunctionLatencySummary | undefined }) {
  if (!summary?.series.length) return null;
  return <section aria-labelledby="function-latency-heading">
    <h2 id="function-latency-heading">Function latency</h2>
    <p className="usage-note">Measured by the Kernel, from admission to the response decision. Total time includes queueing and cold starts. These figures cover retained calls in the selected period, up to {count.format(summary.maxSamples)} recent calls across this app.</p>
    <div className="ui-table-scroll"><table className="ui-table">
      <caption>Each row has one function version and time limit. The p95 is the time within which 95% of its recorded calls finished.</caption>
      <thead><tr><th scope="col">Function</th><th scope="col">Time limit used at p95</th><th scope="col">Total latency</th><th scope="col">Recorded calls</th><th scope="col">Breakdown</th></tr></thead>
      <tbody>{summary.series.map((row) => <tr key={`${row.name}/${row.version}/${row.limitMs}`}>
        <th scope="row">{row.name}<small title={row.version}>{row.version.slice(0, 12)}</small></th>
        <td data-label="Time limit used at p95">
          <div className="function-latency-budget">
          <strong className={row.timeouts > 0 || row.budgetP95Percent >= 100 ? "usage-failure" : undefined}>{milliseconds.format(row.budgetP95Percent)}%</strong>
          <small>{ms(row.total.p95Ms)} of {ms(row.limitMs)}</small>
          <progress aria-label={`${row.name} p95 time limit used`} aria-valuetext={`${milliseconds.format(row.budgetP95Percent)}%`} max={100} value={Math.min(100, Math.max(0, row.budgetP95Percent))} />
          </div>
        </td>
        <td data-label="Total latency">p50 {ms(row.total.p50Ms)}<small>p95 {ms(row.total.p95Ms)}</small><small>p99 {ms(row.total.p99Ms)}</small><small>Max {ms(row.total.maxMs)}</small></td>
        <td data-label="Recorded calls">{count.format(row.samples)} calls
          <small className={row.timeouts > 0 ? "usage-failure" : undefined}>{count.format(row.timeouts)} timeouts</small>
          <small>{count.format(row.coldStarts)} cold starts</small>
          {row.failures > 0 && <small>{count.format(row.failures)} failures or refusals</small>}
          {row.uncertain > 0 && <small className="usage-failure">{count.format(row.uncertain)} {row.uncertain === 1 ? "write" : "writes"} with an unconfirmed result</small>}
          {row.replays > 0 && <small>{count.format(row.replays)} retries answered from a saved result</small>}
        </td>
        <td data-label="Breakdown"><details><summary>Stage p95 times</summary>
          <p>Queue {ms(row.queue.p95Ms)}<br />Initialization {ms(row.initialization.p95Ms)}<br />Execution {ms(row.execution.p95Ms)}</p>
          <small>Stage percentiles do not add up to the total percentile. Initialization is zero for warm calls. Execution includes data operations.</small>
        </details></td>
      </tr>)}</tbody>
    </table></div>
    {summary.seriesTruncated && <p className="usage-note">Showing the {summary.seriesLimit} most recently active function/version/limit combinations.</p>}
    {summary.prunedThrough !== null && <p className="usage-note">Older latency samples have been removed. Percentiles and counts describe the retained samples, not all historical calls.</p>}
  </section>;
}
