import { Loader2, X } from "lucide-react";
import { cloneElement, useId, type ComponentProps, type CSSProperties, type HTMLAttributes, type ReactElement, type ReactNode, type RefObject } from "react";
import { useModalDialog } from "../useModalDialog";
import { LOADING_DELAY_MS } from "../loading-hold";

const classes = (...values: (string | false | undefined)[]) => values.filter(Boolean).join(" ");

export type ButtonProps = ComponentProps<"button"> & {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md" | "icon";
  loading?: boolean;
};

export function Button({ variant = "secondary", size = "md", loading = false, disabled, children, className, type = "button", ...props }: ButtonProps) {
  return <button {...props} type={type} disabled={disabled || loading} aria-busy={loading || undefined} className={classes("ui-button", `ui-button-${variant}`, `ui-button-${size}`, className)}>{loading && <Loader2 className="spin" size={14} />}{children}</button>;
}

export function IconButton({ label, children, ...props }: Omit<ButtonProps, "size"> & { label: string; children: ReactNode }) {
  return <Button variant="ghost" {...props} size="icon" aria-label={label} title={label}>{children}</Button>;
}

export function Input({ className, ...props }: ComponentProps<"input">) {
  return <input {...props} className={classes("ui-input", className)} />;
}

export function Select({ className, ...props }: ComponentProps<"select">) {
  return <select {...props} className={classes("ui-select", className)} />;
}

export function Textarea({ className, ...props }: ComponentProps<"textarea">) {
  return <textarea {...props} className={classes("ui-textarea", className)} />;
}

export function Field({ label, help, children }: { label: string; help?: string; children: ReactElement<{ id?: string; "aria-describedby"?: string; "aria-invalid"?: boolean | "true" | "false" }> }) {
  const generatedId = useId();
  const id = children.props.id ?? generatedId;
  const helpId = help ? `${generatedId}-help` : undefined;
  const describedBy = [children.props["aria-describedby"], helpId].filter(Boolean).join(" ") || undefined;
  const invalid = children.props["aria-invalid"] === true || children.props["aria-invalid"] === "true";
  return <div className="ui-field"><label htmlFor={id}>{label}</label>{cloneElement(children, { id, "aria-describedby": describedBy })}{help && <small id={helpId} className={invalid ? "is-error" : undefined}>{help}</small>}</div>;
}

export function Badge({ tone = "neutral", className, ...props }: HTMLAttributes<HTMLSpanElement> & { tone?: "neutral" | "accent" | "success" | "warning" | "danger" }) {
  return <span {...props} className={classes("ui-badge", `ui-badge-${tone}`, className)} />;
}

export function Card({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return <div {...props} className={classes("ui-card", className)} />;
}

export function EmptyState({ icon, title, description, action, className }: { icon?: ReactNode; title: string; description: string; action?: ReactNode; className?: string }) {
  return <div className={classes("ui-empty", className)}>{icon && <span className="ui-empty-icon">{icon}</span>}<h3>{title}</h3><p>{description}</p>{action}</div>;
}

export type DialogProps = {
  open?: boolean;
  title: string;
  description?: string;
  showTitle?: boolean;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  className?: string;
  initialFocus?: RefObject<HTMLElement | null>;
};

export function Dialog({ open = true, title, description, showTitle = true, onClose, children, footer, className, initialFocus }: DialogProps) {
  const dialog = useModalDialog({ open, onClose, initialFocus });
  if (!open) return null;
  return <div className="ui-dialog-backdrop" onClick={onClose}>
    <section className={classes("ui-dialog", className)} role="dialog" aria-modal="true" aria-label={title} ref={dialog.ref} onKeyDown={dialog.onKeyDown} onClick={(event) => event.stopPropagation()}>
      <IconButton label={`Close ${title}`} className="ui-dialog-close" onClick={onClose}><X size={16} /></IconButton>
      {showTitle && <header className="ui-dialog-heading"><h2>{title}</h2>{description && <p>{description}</p>}</header>}
      {children}
      {footer && <footer className="ui-dialog-footer">{footer}</footer>}
    </section>
  </div>;
}

export type SkeletonProps = { width?: number | string; height?: number | string; radius?: number | string; className?: string };

/** A quiet placeholder shape. Give it the size of the content that replaces it so nothing moves. */
export function Skeleton({ width = "100%", height = 14, radius, className }: SkeletonProps) {
  return <span aria-hidden="true" className={classes("ui-skeleton", className)} style={{ width, height, borderRadius: radius }} />;
}

/**
 * One loading surface: a single polite status with one label, and skeleton
 * shapes that hold the layout. The shapes are in the page from the first
 * paint but fade in after the loading delay, so a fast load shows nothing.
 * `sinceNavigation` counts that delay from the page load, so a skeleton
 * remounted by the next boot stage does not blink.
 */
export function LoadingBlock({ label, rows = 3, children, className, sinceNavigation = false }: { label: string; rows?: number; children?: ReactNode; className?: string; sinceNavigation?: boolean }) {
  const delay = sinceNavigation && typeof performance !== "undefined" ? Math.round(LOADING_DELAY_MS - performance.now()) : LOADING_DELAY_MS;
  return <div role="status" aria-busy="true" className={classes("ui-loading", className)} style={{ "--ui-loading-delay": `${delay}ms` } as CSSProperties}>
    <span className="sr-only">{label}</span>
    <div className="ui-loading-shapes" aria-hidden="true">
      {children ?? Array.from({ length: rows }, (_, index) => <Skeleton key={index} height={16} width={index === rows - 1 && rows > 1 ? "62%" : "100%"} />)}
    </div>
  </div>;
}
