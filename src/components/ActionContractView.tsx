import { useRef } from "react";
import type { ActionContractState } from "./useActionContract";

/**
 * The generated Action Contract, read-only, where a file would be. It says
 * where it comes from, so nobody edits it expecting the change to stick.
 */
export function ActionContractView({ state }: { state: ActionContractState }) {
  const text = state.ok ? state.markdown : state.reason;
  const lines = text.split("\n").length;
  const gutter = useRef<HTMLDivElement>(null);
  return (
    <>
      <div className="action-readout-block">
        <p className={`action-check ${state.ok && state.warnings.length === 0 ? "" : "is-refused"}`}>
          {state.ok
            ? `Generated from actions.json and schema.sql, so it cannot be edited here. It describes ${state.actions} action${state.actions === 1 ? "" : "s"} for a second backend and travels in the export.`
            : "Generated from actions.json and schema.sql. Fix actions.json to see it."}
        </p>
        {state.ok && state.warnings.length > 0 && (
          <ul className="action-readout">
            {state.warnings.map((warning) => <li key={warning.message}><strong className="action-limit">{warning.message}</strong></li>)}
          </ul>
        )}
      </div>
      <div className="code-area">
        <div className="line-numbers" aria-hidden="true" ref={gutter}>
          {Array.from({ length: lines }, (_, index) => <span key={index}>{index + 1}</span>)}
        </div>
        <textarea
          value={text}
          readOnly
          spellCheck={false}
          aria-label="action-contract.md, generated, read only"
          onScroll={(event) => { if (gutter.current) gutter.current.scrollTop = event.currentTarget.scrollTop; }}
        />
      </div>
    </>
  );
}
