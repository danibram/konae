import { useEffect, useState, type RefObject } from "react";
import type { MiniAppRuntimeHost, PreviewFailure } from "../runtime/host";

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
