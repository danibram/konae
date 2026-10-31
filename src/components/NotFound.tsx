import { ArrowLeft, Box } from "lucide-react";
import { Card } from "./ui";

/** An address that names no page, or a project this workspace does not hold. */
export function NotFound({ what, path }: { what: "page" | "project"; path: string }) {
  const project = what === "project";
  return (
    <div className="platform-page">
      <header className="platform-header">
        <a className="platform-brand" href="/"><span><Box size={16} /></span>konae</a>
        <a className="ui-button ui-button-sm platform-back" href="/"><ArrowLeft size={14} /> Back to projects</a>
      </header>
      <main className="platform-main">
        <Card className="platform-card">
          <h1>{project ? "Project not found" : "Page not found"}</h1>
          <p className="platform-lead">
            {project
              ? "This workspace has no project at that address. It may have been deleted, or it belongs to another owner account."
              : "Konae has no page at that address."}
          </p>
          <p className="platform-note"><code>{path}</code></p>
          <div className="platform-actions">
            <a className="ui-button ui-button-primary" href="/"><ArrowLeft size={14} /> Back to projects</a>
          </div>
        </Card>
      </main>
    </div>
  );
}
