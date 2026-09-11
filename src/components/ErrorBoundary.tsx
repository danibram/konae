import { CircleAlert, RefreshCw, RotateCcw } from "lucide-react";
import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./ui";

type State = { error: Error | null };

/**
 * The last line of defence. A render error anywhere below shows what broke and
 * a way out, instead of a blank page. Project files are saved separately, so
 * reloading does not lose work that already reached local storage.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("Littlebox editor crashed", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <main className="app-crash" role="alert">
        <span className="app-crash-icon"><CircleAlert size={22} /></span>
        <h1>The editor hit an error</h1>
        <p>Your project files are saved in this browser. Try again to keep working, or reload the editor.</p>
        <pre>{error.message}</pre>
        <div className="app-crash-actions">
          <Button variant="primary" onClick={() => this.setState({ error: null })}><RotateCcw size={14} /> Try again</Button>
          <Button onClick={() => window.location.reload()}><RefreshCw size={14} /> Reload editor</Button>
          <a className="ui-button" href="/">Back to projects</a>
        </div>
      </main>
    );
  }
}
