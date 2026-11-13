import { Download, FolderOpen, Globe2, LockKeyhole, MoreHorizontal, Pencil, Plus, Settings2, Trash2 } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { readOwnerProjectCount } from "../lib/project-sources";
import { defaultTemplate, templateChoices, type TemplateId } from "../lib/templates";
import type { OwnerSession } from "./owner-session";
import { WorkspaceNav } from "./WorkspaceNav";
import { Actions, Steps, Alert, Avatar, Composer, Identity, Lede, List, ListItem, Page, PageHeader, SectionHead, Tick, Tile, Tiles, Badge, BarChart, Breadcrumb, Button, Card, Checkbox, ChartLegend, Dialog, DonutChart, EmptyState, Field, IconButton, Input, Kbd, LineChart, LoadingBlock, Menu, Progress, Radio, Segmented, Select, Skeleton, Sparkline, StatStrip, Switch, Table, Tabs, Textarea, Tooltip, ToastRegion, type ToastItem } from "./ui";

function Sample({ id, title, note, children }: { id: string; title: string; note?: string; children: ReactNode }) {
  return <section className="ds-sample" id={id} aria-labelledby={`${id}-title`}><h2 id={`${id}-title`}>{title}</h2>{note && <p className="ds-note">{note}</p>}{children}</section>;
}

const sections = [["foundations", "Foundations"], ["actions", "Actions"], ["fields", "Fields"], ["navigation", "Navigation"], ["feedback", "Feedback"], ["data", "Data"], ["charts", "Charts"], ["patterns", "App patterns"], ["surfaces", "Surfaces"], ["loading", "Loading"], ["dialogs", "Dialogs"]];

/** Thirty days of fixed numbers, so the gallery looks the same on every visit. */
const days = Array.from({ length: 30 }, (_, index) => {
  const date = new Date(Date.UTC(2026, 9, 11 + index));
  const wave = 1 + Math.sin(index / 3.1) * 0.35 + (index % 7 === 5 ? -0.5 : 0) + index * 0.012;
  const ok = Math.round(120 * wave + (index * 37) % 23);
  const refused = Math.round(9 * wave + (index * 11) % 7);
  const failed = index % 9 === 4 ? 6 : index % 5 === 0 ? 2 : 0;
  return { key: date.toISOString().slice(0, 10), label: new Intl.DateTimeFormat("en", { month: "short", day: "numeric", timeZone: "UTC" }).format(date), ok, refused, failed };
});
const outcomeNames: Record<string, string> = { ok: "Completed", refused: "Refused", failed: "Failed" };
const barPoints = days.map((day) => ({ key: day.key, label: day.label, value: day.ok + day.refused + day.failed, parts: { ok: day.ok, ...(day.refused ? { refused: day.refused } : {}), ...(day.failed ? { failed: day.failed } : {}) } }));
const viewSeries = [{ id: "views", label: "Page views", values: days.map((day) => Math.round(day.ok * 2.4)) }, { id: "visits", label: "Sessions", values: days.map((day) => Math.round(day.ok * 0.9)) }];
const sum = (values: number[]) => values.reduce((a, b) => a + b, 0);

const sampleApps: [string, string, string, string][] = [["Tiny Tasks", "Published", "1,284", "+12%"], ["Guest list", "Published", "402", "-3%"], ["Weekend planner", "Draft", "-", "-"]];

const formatTotal = (values: number[]) => new Intl.NumberFormat("en").format(sum(values));

function SwitchDemo() {
  const [on, setOn] = useState(true);
  const [mail, setMail] = useState(false);
  return <><Switch label="Published" checked={on} onChange={setOn} /><Switch label="Email me about refusals" checked={mail} onChange={setMail} /><Switch label="Unavailable" checked={false} onChange={() => undefined} disabled /></>;
}

function TinyTasksDemo() {
  const [draft, setDraft] = useState("");
  const [rows, setRows] = useState([{ id: 3, title: "Book the venue", done: true }, { id: 2, title: "Send the invitations", done: false }, { id: 1, title: "Ask Ana about the playlist", done: false }]);
  const add = () => { const title = draft.trim(); if (!title) return; setRows((list) => [{ id: Math.max(0, ...list.map((row) => row.id)) + 1, title, done: false }, ...list]); setDraft(""); };
  return <Page className="ds-pattern-page">
    <PageHeader eyebrow="Shared list" title="Tiny Tasks" actions={<Identity name="Ada Lovelace"><Button size="sm" variant="ghost">Sign out</Button></Identity>} />
    <Lede>Anyone with the link sees the same list.</Lede>
    <Actions><Button variant="primary">Share the list</Button><Button>Print it</Button></Actions>
    <Composer label="New task" placeholder="Add one small thing" action="Add" value={draft} onChange={setDraft} onSubmit={add} />
    <SectionHead label="Today" meta={`${rows.length} items`} />
    <List>{rows.map((row) => <ListItem key={row.id} done={row.done} lead={<Tick pressed={row.done} label={`Toggle ${row.title}`} onToggle={() => setRows((list) => list.map((item) => item.id === row.id ? { ...item, done: !item.done } : item))} />} title={row.title} detail={`#${row.id}`} end={<Button size="sm" variant="ghost" onClick={() => setRows((list) => list.filter((item) => item.id !== row.id))}>Remove</Button>} />)}</List>
    <SectionHead label="Pinboard" meta="Add file" />
    <Tiles small><Tile>notes.pdf</Tile><Tile>menu.txt</Tile><Tile>photo.png</Tile></Tiles>
    <SectionHead label="Images" meta="3 of 12" />
    <Tiles><Tile caption="Studio" end={<Button size="sm" variant="ghost">Remove</Button>}>Image</Tile><Tile caption="Garden">Image</Tile><Tile caption="Kitchen">Image</Tile></Tiles>
  </Page>;
}

function SegmentedDemo() {
  const [value, setValue] = useState(30);
  return <Segmented label="Period" value={value} onChange={setValue} options={[{ value: 7, label: "7 days" }, { value: 30, label: "30 days" }, { value: 90, label: "90 days" }]} />;
}

function ToastDemo() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const next = useRef(1);
  const push = (tone: ToastItem["tone"], message: string) => {
    const id = next.current++;
    setToasts((list) => [...list, { id, tone, message }]);
    window.setTimeout(() => setToasts((list) => list.filter((toast) => toast.id !== id)), 5000);
  };
  return <><div className="ds-row"><Button onClick={() => push("success", "Published to tiny-tasks.example.com")}>Show success</Button><Button onClick={() => push("danger", "The Kernel refused this change")}>Show error</Button></div><ToastRegion toasts={toasts} onDismiss={(id) => setToasts((list) => list.filter((toast) => toast.id !== id))} /></>;
}

export function DesignSystem({ ownerSession }: { ownerSession: OwnerSession }) {
  const [dialogOpen, setDialogOpen] = useState(false);
  const [sampleTemplate, setSampleTemplate] = useState<TemplateId>(defaultTemplate);
  const nameRef = useRef<HTMLInputElement>(null);
  const [projectCount, setProjectCount] = useState<number | undefined>();
  const [variant, setVariant] = useState<"brutal" | "clean">(() => {
    const asked = new URLSearchParams(window.location.search).get("variant") ?? localStorage.getItem("konae:ds-variant");
    return asked === "clean" ? "clean" : "brutal";
  });
  const chooseVariant = (next: "brutal" | "clean") => { setVariant(next); try { localStorage.setItem("konae:ds-variant", next); } catch { /* the choice lasts until reload */ } };

  useEffect(() => {
    if (ownerSession.state !== "signed-in") {
      setProjectCount(undefined);
      return;
    }
    let active = true;
    void readOwnerProjectCount(ownerSession.ownerId)
      .then((count) => { if (active) setProjectCount(count); })
      .catch(() => { if (active) setProjectCount(undefined); });
    return () => { active = false; };
  }, [ownerSession]);

  return (
    <div className="account-home" data-ds={variant === "clean" ? "clean" : undefined}>
      <WorkspaceNav active="design-system" projectCount={projectCount} ownerSession={ownerSession} />
      <main className="account-main">
        <section className="account-content ds-workspace">
          <header className="account-hero ds-hero">
            <p className="account-kicker"><span>◆</span> Konae UI</p>
            <h1>Design system</h1>
            <p>Component examples. These controls do not change projects or upload files. Open a project to work with its files.</p>
            <div className="ds-variant"><span id="ds-variant-label">Variant</span><Segmented label="Design system variant" value={variant} onChange={chooseVariant} options={[{ value: "brutal", label: "Brutal" }, { value: "clean", label: "Clean" }]} /><small>Same components, different tokens. Clean sets <code>data-ds="clean"</code> on any element.</small></div>
          </header>
          <div className="ds-page-inner">
            <nav className="ds-index" aria-label="On this page">{sections.map(([id, label]) => <a key={id} href={`#${id}`}>{label}</a>)}</nav>
            <Sample id="foundations" title="Foundations" note="Every colour is a token. Light and dark come from the same names, so nothing in a component picks a hex. The theme follows the system.">
              <div className="ds-swatches">{[["canvas", "Page background"], ["surface", "Cards and dialogs"], ["subtle", "Quiet fill"], ["border", "Hairlines"], ["stroke", "Strokes"], ["text", "Writing"], ["muted", "Secondary writing"], ["accent", "Primary"], ["orange", "Emphasis"], ["success", "Saved"], ["warning", "Careful"], ["danger", "Destructive"], ["info", "Information"]].map(([token, job]) => <div key={token}><i style={{ background: `var(--ds-${token})` }} /><span>{job}</span><code>--ds-{token}</code></div>)}</div>
              <h3 className="ds-sub">Chart series</h3>
              <p className="ds-note">Five hues in a fixed order, validated for colour-blind readers in both themes. A series keeps its colour when others are filtered out.</p>
              <ChartLegend items={[1, 2, 3, 4, 5].map((n) => ({ label: `Series ${n}`, color: `var(--ds-s${n})` }))} />
              <div className="ds-shadows">
                <div className="ds-shadow-chip ds-shadow-sm"><span>Shadow sm</span><code>--ds-shadow-sm</code></div>
                <div className="ds-shadow-chip ds-shadow-md"><span>Shadow</span><code>--ds-shadow</code></div>
                <div className="ds-shadow-chip ds-shadow-lg"><span>Shadow lg</span><code>--ds-shadow-lg</code></div>
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
                <small>Brutal sets Bricolage Grotesque with JetBrains Mono labels. Clean uses the system sans for both.</small>
              </div>
            </Sample>
            <Sample id="actions" title="Actions"><div className="ds-row"><Button variant="primary"><Plus size={15} /> Create project</Button><Button>Cancel</Button><Button>View details</Button><Button variant="ghost">Skip for now</Button><Button variant="danger">Delete project</Button><Button disabled>Unavailable</Button><Button variant="primary" loading>Saving</Button><IconButton label="Settings"><Settings2 size={16} /></IconButton></div>
              <div className="ds-row ds-gap"><Button size="sm" variant="primary">Small primary</Button><Button size="sm">Small</Button><Badge tone="accent">Public</Badge><Badge tone="success">Saved</Badge><Badge tone="warning">Stops at 100</Badge><Badge>Starter</Badge><Badge tone="danger">Not saved</Badge></div></Sample>
            <Sample id="fields" title="Fields"><div className="ds-fields"><Field label="Project name" help="Choose a name you can recognize later."><Input placeholder="Weekend planner" /></Field><Field label="Visibility"><Select defaultValue="private"><option value="private">Private</option><option value="public">Public</option></Select></Field><Field label="Search"><Input type="search" placeholder="Search projects" /></Field><Field label="Unavailable field"><Input disabled value="Kept with this app" readOnly /></Field><Field label="Validation error" help="A project name is required."><Input aria-invalid="true" placeholder="Project name" /></Field><Field label="Description"><Textarea rows={3} placeholder="What will this app do?" /></Field></div>
              <div className="ds-controls">
                <div><h3 className="ds-sub">Checkbox</h3><div className="ds-stack"><Checkbox label="Anyone with the link can read" defaultChecked /><Checkbox label="Email me when a row changes" /><Checkbox label="Unavailable" disabled /></div></div>
                <div><h3 className="ds-sub">Radio</h3><div className="ds-stack"><Radio name="ds-who" label="Only me" defaultChecked /><Radio name="ds-who" label="People I invite" /><Radio name="ds-who" label="Anyone" /></div></div>
                <div><h3 className="ds-sub">Switch</h3><div className="ds-stack"><SwitchDemo /></div></div>
                <div><h3 className="ds-sub">Segmented</h3><SegmentedDemo /></div>
              </div></Sample>
            <Sample id="navigation" title="Navigation" note="The project list sidebar, in the states it takes. The frame is a demo, so its links stay on this page.">
              <div className="ds-nav-frames" onClickCapture={(event) => { if ((event.target as HTMLElement).closest("a")) event.preventDefault(); }}>
                <div className="ds-frame ds-frame-wide"><span>Project list, signed in</span><div className="account-home ds-frame-home"><WorkspaceNav active="usage" projectCount={7} ownerSession={{ state: "signed-in", ownerId: "demo", email: "ada@example.com" }} />
                  <div className="ds-frame-page"><Breadcrumb items={[{ label: "Usage", href: "/usage" }, { label: "Tiny Tasks" }]} /><h3>Tiny Tasks</h3><StatStrip items={[{ label: "Page views", value: "12,480", delta: "+12%", trend: "up" }, { label: "Executions", value: "4,999", delta: "+4%", trend: "up" }, { label: "Refused", value: "407", delta: "+18%", trend: "up", upIsGood: false }]} /></div></div></div>
                <div className="ds-frame ds-frame-narrow"><span>Signed out</span><div className="account-home ds-frame-home"><WorkspaceNav active="projects" ownerSession={{ state: "signed-out" }} /></div></div>
                <div className="ds-frame ds-frame-narrow"><span>Checking the session</span><div className="account-home ds-frame-home"><WorkspaceNav active="design-system" ownerSession={{ state: "checking" }} /></div></div>
              </div>
              <h3 className="ds-sub">Tabs</h3>
              <Tabs label="Example tabs" items={[
                { id: "rows", label: "Rows", count: 42, panel: <Table><thead><tr><th scope="col">App</th><th scope="col">Status</th><th scope="col">Views</th></tr></thead><tbody>{sampleApps.slice(0, 2).map(([name, status, views]) => <tr key={name}><th scope="row">{name}</th><td>{status}</td><td>{views}</td></tr>)}</tbody></Table> },
                { id: "rules", label: "Rules", panel: <p className="ds-note">Who may read a row is a decision of the app, written next to the data.</p> },
                { id: "history", label: "History", count: 3, panel: <p className="ds-note">Three published versions. The newest is live.</p> },
              ]} />
              <div className="ds-row ds-gap">
                <Breadcrumb items={[{ label: "Usage", href: "/usage" }, { label: "Tiny Tasks", href: "/usage" }, { label: "Page views" }]} />
                <Menu label="Project actions" trigger={<MoreHorizontal size={16} />} items={[{ label: "Rename", icon: <Pencil size={14} />, onSelect: () => undefined }, { label: "Export", icon: <Download size={14} />, onSelect: () => undefined }, { label: "Delete", icon: <Trash2 size={14} />, danger: true, onSelect: () => undefined }]} />
                <Tooltip text="Opens the published address"><Button>Hover or focus me</Button></Tooltip>
                <span className="ds-note">Press <Kbd>Esc</Kbd> to close a dialog, <Kbd>←</Kbd> <Kbd>→</Kbd> to move between tabs.</span>
              </div>
            </Sample>
            <Sample id="feedback" title="Feedback">
              <div className="ds-stack ds-wide">
                <Alert title="Your app is published">It answers at tiny-tasks.example.com.</Alert>
                <Alert tone="success" title="Saved">Anyone with the link can now add a row.</Alert>
                <Alert tone="warning" title="3 email notices were not sent">The actions completed. Only the email is missing.</Alert>
                <Alert tone="danger" title="Usage could not be refreshed" action={<Button>Try again</Button>}>The Kernel answered 503.</Alert>
              </div>
              <div className="ds-fields ds-gap"><div><h3 className="ds-sub">Progress</h3><div className="ds-stack"><Progress label="Rows used" value={42} /><Progress label="Rows used" value={78} tone="warning" /><Progress label="Rows used" value={100} tone="danger" /></div></div>
                <div><h3 className="ds-sub">Toast</h3><ToastDemo /></div></div>
            </Sample>
            <Sample id="data" title="Data" note="Numbers sit in tabular figures. A table becomes stacked rows on a narrow screen.">
              <StatStrip items={[
                { label: "Page views", value: "12,480", delta: "+12%", trend: "up", spark: <Sparkline label="Page views, last 14 days" values={viewSeries[0].values.slice(-14)} /> },
                { label: "Executions", value: formatTotal(barPoints.map((p) => p.value)), delta: "+4%", trend: "up", spark: <Sparkline label="Executions, last 14 days" values={barPoints.map((p) => p.value).slice(-14)} /> },
                { label: "Refused", value: String(sum(days.map((d) => d.refused))), delta: "+18%", trend: "up", upIsGood: false },
                { label: "Failed", value: String(sum(days.map((d) => d.failed))), delta: "same", trend: "flat" },
              ]} />
              <div className="ds-gap">
                <Table caption="Published apps in the last 30 days.">
                  <thead><tr><th scope="col">App</th><th scope="col">Status</th><th scope="col">Page views</th><th scope="col">Change</th></tr></thead>
                  <tbody>{sampleApps.map(([name, status, views, change]) => <tr key={name}><th scope="row"><span className="ds-person"><Avatar name={name} size={26} />{name}</span></th><td data-label="Status"><Badge tone={status === "Published" ? "success" : "neutral"}>{status}</Badge></td><td data-label="Page views">{views}</td><td data-label="Change">{change}</td></tr>)}</tbody>
                </Table>
              </div>
            </Sample>
            <Sample id="charts" title="Charts" note="The same charts Usage draws. Hover or tab to a mark for its numbers; every chart has a table behind it.">
              <Card className="ds-chart-card"><BarChart title="Executions per day" unit="executions" points={barPoints} partLabel={(part) => outcomeNames[part] ?? part} partColor={(part) => part === "failed" ? "var(--ds-danger)" : part === "refused" ? "var(--ds-warning-fill)" : undefined} stacked empty="Nothing recorded." keyHeading="Day (UTC)" /></Card>
              <div className="ds-fields ds-gap">
                <Card className="ds-chart-card"><LineChart title="Visits" unit="visits" labels={days.map((d) => d.label)} series={viewSeries} area empty="Nothing recorded." /></Card>
                <Card className="ds-chart-card"><DonutChart title="Where views come from" unit="views" slices={[{ id: "home", label: "Home", value: 5120 }, { id: "list", label: "List", value: 3304 }, { id: "about", label: "About", value: 1450 }, { id: "other", label: "Other", value: 640 }]} empty="Nothing recorded." /></Card>
              </div>
              <Card className="ds-chart-card ds-gap"><BarChart title="A quiet period" unit="views" points={days.slice(0, 7).map((d) => ({ key: d.key, label: d.label, value: 0 }))} empty="No page views recorded in this period." /></Card>
            </Sample>
            <Sample id="patterns" title="App patterns" note="What a small app screen is made of. Published apps get the same classes in their styles.css, in whichever variant the app picked.">
              <Card className="ds-pattern-frame"><TinyTasksDemo /></Card>
              <Card className="ds-pattern-frame"><div className="ui-page ds-pattern-page"><SectionHead label="Start" /><Steps items={[{ title: "Get an invitation", detail: "The person who runs the installation sends a code." }, { title: "Describe the app", detail: "Or write its files yourself." }, { title: "Open it and use it" }]} /></div></Card>
              <p className="ds-note">A page that stands on its own, such as an installation's home, adds <code>ui-skip</code> for the skip link and <code>ui-bar</code> for a top bar with the name and a few links.</p>
            </Sample>
            <Sample id="surfaces" title="Surfaces and empty states"><Card><EmptyState icon={<FolderOpen size={24} />} title="No files yet" description="Add one when the app needs it. Export takes it along." action={<Button variant="primary"><Plus size={14} /> Upload file</Button>} /></Card></Sample>
            <Sample id="loading" title="Loading"><div className="ds-fields"><Card className="ui-loading-card"><LoadingBlock label="Loading example rows" rows={3} /></Card><Card className="ui-loading-card"><div className="ui-loading-shapes" aria-hidden="true"><Skeleton width={32} height={32} radius="50%" /><Skeleton width="60%" height={14} /><Skeleton height={36} /></div></Card></div><p className="ds-note">A skeleton has the size of what replaces it. It fades in after 200 ms, so a fast load shows nothing, and it stays at least 300 ms once shown. Each surface has one status label for screen readers and no changing sentence. Reduced motion stops the shimmer.</p></Sample>
            <Sample id="dialogs" title="Dialogs"><Button onClick={() => setDialogOpen(true)}>Open example dialog</Button><p className="ds-note">Dialogs trap focus, close with Escape and return focus to their trigger.</p></Sample>
          </div>
        </section>
      </main>
      <Dialog open={dialogOpen} className="new-project" title="Name this app" description="Pick a name, then how it starts." initialFocus={nameRef} onClose={() => setDialogOpen(false)} footer={<><Button onClick={() => setDialogOpen(false)}>Cancel</Button><Button variant="primary" disabled onClick={() => setDialogOpen(false)}>Create</Button></>}>
        <div className="ui-dialog-body new-project-dialog">
          <Field label="Project name" help="Choose a name you can recognize later."><Input ref={nameRef} placeholder="" /></Field>
          <fieldset className="start-cards">
            <legend>Start with</legend>
            <p className="start-note">Each one runs as it is. Describe what to change next.</p>
            {templateChoices.map((choice) => (
              <label key={choice.id} className={sampleTemplate === choice.id ? "selected" : ""}>
                <input type="radio" name="sample-template" value={choice.id} checked={sampleTemplate === choice.id} onChange={() => setSampleTemplate(choice.id)} />
                <span className="start-title"><strong>{choice.name}</strong><Badge tone={choice.visibility === "public" ? "success" : "accent"}>{choice.visibility === "public" ? <Globe2 size={11} /> : <LockKeyhole size={11} />}{choice.visibility === "public" ? "Public" : "Private"}</Badge></span>
                <small>{choice.sentence}</small>
              </label>
            ))}
          </fieldset>
        </div>
      </Dialog>
    </div>
  );
}
