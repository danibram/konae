import { useEffect, useRef, type KeyboardEvent as ReactKeyboardEvent, type RefObject } from "react";

export type ModalDialog = {
  ref: RefObject<HTMLElement | null>;
  onKeyDown: (event: ReactKeyboardEvent<HTMLElement>) => void;
};

type Options = {
  open?: boolean;
  onClose: () => void;
  initialFocus?: RefObject<HTMLElement | null>;
};

const openDialogs: object[] = [];

export function useModalDialog({ open = true, onClose, initialFocus }: Options): ModalDialog {
  const ref = useRef<HTMLElement>(null);
  const restore = useRef<HTMLElement | null>(null);
  const close = useRef(onClose);
  close.current = onClose;

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
