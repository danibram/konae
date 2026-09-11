import { useEffect, useState, type RefObject } from "react";
import type { MiniAppRuntimeHost, PreviewFailure } from "../runtime/host";

/**
 * What the preview cannot run, meaning a broken actions.json or schema.sql, and
 * what the running app threw. File failures arrive through the host's diagnostics
 * subscription; runtime errors have no event of their own, so they are read
 * once a second and only stored when they change.
 *
 * `hostKey` changes whenever the bridge builds a new host (a different
 * project), so the subscription follows it.
 */
export function usePreviewProblems(hostRef: RefObject<MiniAppRuntimeHost>, hostKey: string): PreviewFailure[] {
  const [problems, setProblems] = useState<PreviewFailure[]>([]);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    let last = "";
    const read = () => {
      const next = host.previewErrors();
      const signature = JSON.stringify(next.map((problem) => [problem.kind, problem.message, problem.count]));
      if (signature === last) return;
      last = signature;
      setProblems(next);
    };
    const unsubscribe = host.subscribeDiagnostics(read);
    const timer = window.setInterval(read, 1_000);
    return () => {
      unsubscribe();
      window.clearInterval(timer);
    };
  }, [hostRef, hostKey]);

  return problems;
}
