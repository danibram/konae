import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from "react";

export type ModalDialog = {
  /** Goes on the dialog element: the trap needs to know what is inside it. */
  ref: RefObject<HTMLElement | null>;
  /** Goes on the same element, never on window: a dialog above this one keeps its own Escape. */
  onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
};

type Options = {
  /** False while the dialog is mounted but not shown. A dialog that unmounts to close may omit it. */
  open?: boolean;
  onClose: () => void;
  /** Where focus lands when it opens; the first focusable control otherwise. */
  initialFocus?: RefObject<HTMLElement | null>;
};

/**
 * The behaviour `aria-modal="true"` claims: focus moves into the dialog when it
 * opens, Escape closes it, Tab cannot leave it, and focus returns to whatever
 * opened it. Declaring the attribute without doing this leaves a keyboard user
 * operating the editor behind a backdrop that says it is unreachable — so the
 * behaviour lives here, once, rather than in each dialog that remembers to.
 */
/** Open dialogs, newest last. Only the topmost one answers a key that reached the document. */
const openDialogs: object[] = [];

export function useModalDialog({ open = true, onClose, initialFocus }: Options): ModalDialog {
  const ref = useRef<HTMLElement>(null);
  const restore = useRef<HTMLElement | null>(null);
  const close = useRef(onClose);
  close.current = onClose;

  // Focus can still fall out of the dialog, for example when the focused
  // control is removed or disabled. Escape and Tab then reach the document
  // instead of the dialog, so the topmost dialog listens there too.
  useEffect(() => {
    if (!open) return;
    const token = {};
    openDialogs.push(token);
    const onDocumentKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || openDialogs.at(-1) !== token) return;
      const dialog = ref.current;
      if (!dialog || (event.target instanceof Node && dialog.contains(event.target))) return;
      if (event.key === "Escape") {
        event.preventDefault();
        close.current();
      } else if (event.key === "Tab") {
        const focusable = focusableElements(dialog);
        event.preventDefault();
        (event.shiftKey ? focusable.at(-1) : focusable[0])?.focus();
      }
    };
    document.addEventListener("keydown", onDocumentKey);
    return () => {
      document.removeEventListener("keydown", onDocumentKey);
      openDialogs.splice(openDialogs.indexOf(token), 1);
    };
  }, [open]);

  useEffect(() => {
    const restoreFocus = () => {
      const previous = restore.current;
      restore.current = null;
      previous?.focus();
    };
    if (!open) {
      restoreFocus();
      return;
    }
    restore.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    (initialFocus?.current ?? focusableElements(ref.current)[0] ?? ref.current)?.focus();
    // Closing by unmounting has to give focus back too.
    return restoreFocus;
  }, [open, initialFocus]);

  function onKeyDown(event: ReactKeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = focusableElements(ref.current);
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && active === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  }

  return { ref, onKeyDown };
}

const focusableSelector =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function focusableElements(root: HTMLElement | null): HTMLElement[] {
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>(focusableSelector));
}
