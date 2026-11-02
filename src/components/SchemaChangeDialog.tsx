import type { SchemaChange } from "../deployments/release";
import { Button, Dialog } from "./ui";

/**
 * Asks the owner before a release whose schema change loses data. It lists
 * what goes, in the Kernel's words, and the statements the Kernel will run.
 */
export function SchemaChangeDialog({ change, onAnswer }: { change: SchemaChange; onAnswer: (confirmed: boolean) => void }) {
  return <Dialog
    title="This publish deletes data"
    description="schema.sql no longer has what the published database holds. Publishing changes the database to match, and this cannot be undone."
    onClose={() => onAnswer(false)}
    footer={<>
      <Button onClick={() => onAnswer(false)}>Keep the data</Button>
      <Button variant="danger" onClick={() => onAnswer(true)}>Publish and delete</Button>
    </>}
  >
    <div className="ui-dialog-body schema-change">
      <ul className="schema-change-losses">{change.losses.map((loss) => <li key={loss.sentence}>{loss.sentence}</li>)}</ul>
      <details>
        <summary>Statements the Kernel runs, in one transaction</summary>
        <pre>{change.statements.map((statement) => `${statement};`).join("\n")}</pre>
      </details>
    </div>
  </Dialog>;
}
