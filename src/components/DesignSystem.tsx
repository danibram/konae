import { FolderOpen, Plus, Settings2 } from "lucide-react";
import { useMemo, useRef, useState, type ReactNode } from "react";
import { loadWorkspaceState } from "../lib/workspace";
import { useOwnerSession } from "./owner-session";
import { WorkspaceNav } from "./WorkspaceNav";
import { Badge, Button, Card, Dialog, EmptyState, Field, IconButton, Input, Select, Textarea } from "./ui";

function Sample({ title, children }: { title: string; children: ReactNode }) {
  return <section className="ds-sample"><h2>{title}</h2>{children}</section>;
}

export function DesignSystem() {
  const [dialogOpen, setDialogOpen] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);
  const [ownerSession] = useOwnerSession();
  const projectCount = useMemo(() => loadWorkspaceState().projects.length, []);

  return (
    <div className="account-home">
      <WorkspaceNav active="design-system" projectCount={projectCount} ownerSession={ownerSession} />
      <main className="account-main">
        <section className="account-content ds-workspace">
          <header className="account-hero ds-hero">
            <p className="account-kicker"><span>◆</span> Littlebox UI</p>
            <h1>Design system</h1>
            <p>Ink borders, an offset shadow, and one lime primary. These are the pieces an app shares.</p>
          </header>
          <div className="ds-page-inner">
            <Sample title="Foundations">
              <div className="ds-swatches">{[["canvas", "Page background"], ["surface", "Cards and dialogs"], ["subtle", "Quiet fill"], ["border", "Lines"], ["text", "Writing"], ["muted", "Secondary writing"], ["accent", "Primary (acid)"], ["orange", "Emphasis"], ["success", "Saved"], ["danger", "Destructive"]].map(([token, job]) => <div key={token}><i style={{ background: token === "orange" ? "var(--ds-orange)" : `var(--ds-${token})` }} /><span>{job}</span></div>)}</div>
              <div className="ds-shadows">
                <div className="ds-shadow-chip ds-shadow-sm"><span>Shadow sm</span><code>3px 3px 0 ink</code></div>
                <div className="ds-shadow-chip ds-shadow-md"><span>Shadow</span><code>5px 5px 0 ink</code></div>
                <div className="ds-shadow-chip ds-shadow-lg"><span>Shadow lg</span><code>10px 10px 0 ink</code></div>
              </div>
              <div className="ds-panels">
                <div><i style={{ background: "var(--ds-panel-yellow)" }} /><span>Panel yellow</span></div>
                <div><i style={{ background: "var(--ds-panel-coral)" }} /><span>Panel coral</span></div>
                <div><i style={{ background: "var(--ds-lila)" }} /><span>Panel lilac</span></div>
              </div>
              <div className="ds-type">
                <div className="ds-type-display">Tiny Tasks</div>
                <h2>A shared list</h2>
                <p>Anyone with the link can add a row. The list stops at 100.</p>
              </div>
            </Sample>
            <Sample title="Actions"><div className="ds-row"><Button variant="primary"><Plus size={15} /> Create project</Button><Button>Cancel</Button><Button>View details</Button><Button variant="danger">Delete project</Button><Button disabled>Unavailable</Button><Button variant="primary" loading>Saving</Button><IconButton label="Settings"><Settings2 size={16} /></IconButton></div></Sample>
            <Sample title="Status"><div className="ds-row"><Badge tone="accent">Public</Badge><Badge tone="warning">Stops at 100</Badge><Badge>Starter</Badge><Badge tone="danger">Not saved</Badge></div></Sample>
            <Sample title="Fields"><div className="ds-fields"><Field label="Project name" help="Choose a name you can recognize later."><Input placeholder="Weekend planner" /></Field><Field label="Visibility"><Select defaultValue="private"><option value="private">Private</option><option value="public">Public</option></Select></Field><Field label="Search"><Input type="search" placeholder="Search projects" /></Field><Field label="Unavailable field"><Input disabled value="Kept with this app" readOnly /></Field><Field label="Validation error" help="A project name is required."><Input aria-invalid="true" placeholder="Project name" /></Field><Field label="Description"><Textarea rows={3} placeholder="What will this app do?" /></Field></div></Sample>
            <Sample title="Surfaces and empty states"><Card><EmptyState icon={<FolderOpen size={24} />} title="No files yet" description="Add one when the app needs it. Export takes it along." action={<Button variant="primary"><Plus size={14} /> Upload file</Button>} /></Card></Sample>
            <Sample title="Dialogs"><Button onClick={() => setDialogOpen(true)}>Open example dialog</Button><p className="ds-note">Dialogs trap focus, close with Escape and return focus to their trigger.</p></Sample>
          </div>
        </section>
      </main>
      <Dialog open={dialogOpen} title="Name this app" description="Pick a name, then how it starts." initialFocus={nameRef} onClose={() => setDialogOpen(false)} footer={<><Button onClick={() => setDialogOpen(false)}>Cancel</Button><Button variant="primary" disabled onClick={() => setDialogOpen(false)}>Create</Button></>}><div className="ui-dialog-body new-project-dialog"><Field label="Project name" help="Choose a name you can recognize later."><Input ref={nameRef} placeholder="" /></Field><fieldset className="start-cards"><legend>Start with</legend><p className="start-note">Describe is next. A shared list is the start for anything else.</p><label className="selected"><input type="radio" name="sample-template" defaultChecked /><strong>Shared list</strong><small>Anyone with the link can add a row.</small></label><label><input type="radio" name="sample-template" /><strong>Private notes</strong><small>Each person signs in and only sees their own.</small></label></fieldset></div></Dialog>
    </div>
  );
}
