import { useState } from "react";
import { McpError } from "../mcp/client";
import { Button, Dialog } from "./ui";

export function message(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}

export function signedOut(cause: unknown) {
  return cause instanceof McpError && cause.status === 401;
}

type Confirm = { title: string; description: string; action: string; run: () => Promise<unknown> } | null;

export function useConfirm(onSignedOut: () => void) {
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const close = () => { if (!busy) { setConfirm(null); setError(""); } };
  const dialog = confirm && (
    <Dialog title={confirm.title} description={confirm.description} onClose={close} footer={<>
      <Button onClick={close}>Cancel</Button>
      <Button variant="danger" loading={busy} onClick={async () => {
        setBusy(true);
        setError("");
        try {
          await confirm.run();
          setConfirm(null);
        } catch (cause) {
          if (signedOut(cause)) onSignedOut();
          setError(message(cause));
        } finally {
          setBusy(false);
        }
      }}>{confirm.action}</Button>
    </>}>
      {error ? <div className="ui-dialog-body"><div className="access-error" role="alert">{error}</div></div> : <span />}
    </Dialog>
  );
  return { ask: setConfirm, dialog };
}

