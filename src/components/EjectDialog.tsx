import { Download } from "lucide-react";
import { useState } from "react";
import { Button, Dialog } from "./ui";
import type { EjectPlan, EjectReceipt } from "../lib/eject-package";
import "./eject-dialog.css";

type Target = { id: string; name: string; slug: string };

type Props = {
  project: Target | null;
  onClose: () => void;
  /** Plans on the Kernel and returns the checked code package. Nothing changes yet. */
  prepare: (projectId: string, slug: string) => Promise<{ plan: EjectPlan; archive: Uint8Array }>;
  apply: (plan: EjectPlan) => Promise<EjectReceipt>;
  onEjected: (projectId: string, receipt: EjectReceipt) => void;
};

/**
 * The rows are the launch plan's Eject row, word for word, so the dialog
 * says exactly what the Kernel's eject_apply changes and keeps.
 */
export const ejectEffects = [
  ["Code", "The project goes from Managed to External."],
  ["Hosted app and users", "Keeps URL, users, sessions, data, invitations and email service."],
  ["Result", "A conventional project that publishes artifacts through MCP."],
] as const;

export function publishCommands(folder: string, origin: string) {
  return [
    `bun run littlebox -- login --dir ${folder} --url ${origin}`,
    `bun run littlebox -- build --dir ${folder}`,
    `bun run littlebox -- publish --dir ${folder}`,
  ];
}

export function EjectDialog({ project, onClose, prepare, apply, onEjected }: Props) {
  const [step, setStep] = useState<"confirm" | "working" | "done">("confirm");
  const [error, setError] = useState("");
  const [receipt, setReceipt] = useState<EjectReceipt | null>(null);
  const close = () => { if (step === "working") return; setStep("confirm"); setError(""); setReceipt(null); onClose(); };
  if (!project) return null;
  const folder = project.slug || project.id;

  const run = async () => {
    setStep("working");
    setError("");
    try {
      const { plan, archive } = await prepare(project.id, project.slug);
      // The code reaches this browser before the project changes.
      saveFile(new Blob([archive as BlobPart], { type: "application/x-tar" }), `${folder}-code.tar`);
      const done = await apply(plan);
      setReceipt(done);
      setStep("done");
      onEjected(project.id, done);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      setStep("confirm");
    }
  };

  if (step === "done" && receipt) {
    return <Dialog open title={`${project.name} is External`} description="The code downloaded and the project changed." onClose={close} className="eject-dialog" footer={<Button variant="primary" onClick={close}>Done</Button>}>
      <p>{folder}-code.tar holds the sources at saved revision {receipt.sourcesRevision}. In this editor the project opens read-only.{receipt.deployment ? ` ${receipt.deployment.url} keeps serving its users.` : ""}</p>
      <p>Unpack it, then publish from that folder with a Littlebox checkout:</p>
      <pre className="eject-commands">{publishCommands(folder, window.location.origin).join("\n")}</pre>
    </Dialog>;
  }

  return <Dialog
    open
    title={`Eject ${project.name}?`}
    description="Edit this project's code outside Littlebox. This cannot be undone."
    onClose={close}
    className="eject-dialog"
    footer={<>
      <Button onClick={close} disabled={step === "working"}>Cancel</Button>
      <Button variant="primary" loading={step === "working"} onClick={() => void run()}><Download size={14} /> Download the code and eject</Button>
    </>}
  >
    <dl className="eject-effects">
      {ejectEffects.map(([label, text]) => <div key={label}><dt>{label}</dt><dd>{text}</dd></div>)}
    </dl>
    <p>Eject does not cancel hosting, does not export data and does not close your account. Email keeps working while the app is hosted.</p>
    <p>The editor downloads the code and checks it before the project changes. Afterwards Littlebox refuses edits to this project from every tab and agent, and you publish with the Littlebox CLI. Returning to the managed editor creates a new project, with no automatic merge of production data.</p>
    {error && <p className="eject-error" role="alert">Nothing changed. {error}</p>}
  </Dialog>;
}

function saveFile(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}

/** Shown in place of editing and of the agent once a project is External. */
export function EjectedNotice({ projectId, slug, ejectedAt, sourcesRevision }: { projectId: string; slug: string; ejectedAt: number; sourcesRevision: number }) {
  const folder = slug || projectId;
  const when = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" }).format(ejectedAt);
  return <section className="ejected-notice" role="note" aria-label="This project is External">
    <p>This project was ejected on {when} at saved revision {sourcesRevision}. Its code is edited outside Littlebox now, so these files are read-only here and the agent makes no changes. The hosted app keeps running.</p>
    <p>The code went to whoever ejected it, as {folder}-code.tar from this editor or as a folder from <code>bun run littlebox -- eject</code>. To fetch the same code again into an empty folder, run <code>bun run littlebox -- eject --dir {folder} --project {projectId}</code>. Publish from that folder:</p>
    <pre>{publishCommands(folder, window.location.origin).join("\n")}</pre>
  </section>;
}
