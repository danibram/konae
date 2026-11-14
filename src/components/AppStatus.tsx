import { ArrowUpRight } from "lucide-react";
import { useEffect, useState } from "react";
import { exactTime, relativeTime } from "../lib/relative-time";
import "./app-status.css";

export type AppStatusProps = {
  /** The Deployment's address. Without one the app is a draft. */
  url?: string | null;
  /** When the release the app runs was published, in milliseconds: releasedAt() of the Deployment. */
  publishedAt?: number;
  className?: string;
};

/**
 * The state of one app in one line: "Draft", or "Published" with its address
 * (a link that opens in a new tab) and how long ago it was published. The
 * project list and the editor's top bar both draw it, so the two never disagree.
 */
export function AppStatus({ url, publishedAt, className }: AppStatusProps) {
  const publishedTime = useRelativeTime(publishedAt);
  if (!url) {
    return <span className={["app-status", className].filter(Boolean).join(" ")} data-state="draft">
      <span className="app-status-state">Draft</span>
    </span>;
  }
  const address = url.replace(/^https?:\/\//, "").replace(/\/$/, "");
  return <span className={["app-status", className].filter(Boolean).join(" ")} data-state="published">
    <span className="app-status-state">Published</span>
    <a className="app-status-address" href={url} target="_blank" rel="noopener" title={`Open ${address} in a new tab`}>
      <span>{address}</span><ArrowUpRight size={12} aria-hidden="true" />
    </a>
    {publishedAt !== undefined && <time className="app-status-time" dateTime={new Date(publishedAt).toISOString()} title={`Published ${exactTime(publishedAt)}`}>{publishedTime}</time>}
  </span>;
}

/** Updates a relative time while the page stays open. */
export function useRelativeTime(at?: number) {
  const [now, setNow] = useState(Date.now);
  const enabled = at !== undefined;
  useEffect(() => {
    if (!enabled) return;
    const timer = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, [enabled]);
  return at === undefined ? "" : relativeTime(at, now);
}

const undeployedTitle = "The editor holds changes the published app does not run yet. Deploy changes to publish them.";

/**
 * "Changes not deployed", beside the state wherever the state is drawn. With
 * `onDeploy` it is a button that opens Deploy; on a card it is text.
 */
export function UndeployedMarker({ onDeploy, className }: { onDeploy?: () => void; className?: string }) {
  const classes = ["app-status-changes", className].filter(Boolean).join(" ");
  return onDeploy
    ? <button type="button" className={classes} title={undeployedTitle} onClick={onDeploy}>Changes not deployed</button>
    : <span className={classes} title={undeployedTitle}>Changes not deployed</span>;
}
