import { AlertTriangle, CheckCircle2, ChevronRight, Info, Loader2, OctagonAlert, X } from "lucide-react";
import { cloneElement, useEffect, useId, useRef, useState, type ComponentProps, type CSSProperties, type HTMLAttributes, type KeyboardEvent, type ReactElement, type ReactNode, type RefObject } from "react";
import { useModalDialog } from "../useModalDialog";
import { LOADING_DELAY_MS } from "../loading-hold";
import "./ui.css";
import "./variant-clean.css";

export * from "./charts";

const classes = (...values: (string | false | undefined)[]) => values.filter(Boolean).join(" ");

export type ButtonProps = ComponentProps<"button"> & {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md" | "icon";
  loading?: boolean;
};

export function Button({ variant = "secondary", size = "md", loading = false, disabled, children, className, type = "button", ref, ...props }: ButtonProps) {
  return <button {...props} ref={ref} type={type} disabled={disabled || loading} aria-busy={loading || undefined} className={classes("ui-button", `ui-button-${variant}`, `ui-button-${size}`, className)}>{loading && <Loader2 className="spin" size={14} />}{children}</button>;
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

/* ---- Selection ---------------------------------------------------------- */

export type TabItem = { id: string; label: string; count?: number; panel: ReactNode };

/** Tabs with one tab stop: the arrow keys, Home and End move between tabs. */
export function Tabs({ items, label, defaultId }: { items: TabItem[]; label: string; defaultId?: string }) {
  const [current, setCurrent] = useState(defaultId ?? items[0]?.id);
  const base = useId();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const move = (event: KeyboardEvent, index: number) => {
    const next = event.key === "ArrowRight" ? (index + 1) % items.length : event.key === "ArrowLeft" ? (index - 1 + items.length) % items.length : event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : -1;
    if (next < 0) return;
    event.preventDefault();
    setCurrent(items[next].id);
    refs.current[next]?.focus();
  };
  return <div className="ui-tabs">
    <div role="tablist" aria-label={label} className="ui-tablist">
      {items.map((item, index) => <button key={item.id} ref={(node) => { refs.current[index] = node; }} type="button" role="tab" id={`${base}-${item.id}-tab`} aria-controls={`${base}-${item.id}-panel`}
        aria-selected={current === item.id} tabIndex={current === item.id ? 0 : -1} onClick={() => setCurrent(item.id)} onKeyDown={(event) => move(event, index)}>
        {item.label}{item.count !== undefined && <span>{item.count}</span>}</button>)}
    </div>
    {items.map((item) => <div key={item.id} role="tabpanel" id={`${base}-${item.id}-panel`} aria-labelledby={`${base}-${item.id}-tab`} hidden={current !== item.id} className="ui-tabpanel">{current === item.id && item.panel}</div>)}
  </div>;
}

export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean }) {
  return <label className="ui-switch"><button type="button" role="switch" aria-checked={checked} disabled={disabled} onClick={() => onChange(!checked)}><span /></button><span>{label}</span></label>;
}

export function Checkbox({ label, className, ...props }: Omit<ComponentProps<"input">, "type"> & { label: ReactNode }) {
  return <label className={classes("ui-check", className)}><input {...props} type="checkbox" /><span>{label}</span></label>;
}

export function Radio({ label, className, ...props }: Omit<ComponentProps<"input">, "type"> & { label: ReactNode }) {
  return <label className={classes("ui-check ui-radio", className)}><input {...props} type="radio" /><span>{label}</span></label>;
}

/** A short single choice that changes the view, as a period. */
export function Segmented<T extends string | number>({ value, onChange, options, label }: { value: T; onChange: (next: T) => void; options: { value: T; label: string }[]; label: string }) {
  return <div role="radiogroup" aria-label={label} className="ui-segmented">
    {options.map((option) => <button key={option.value} type="button" role="radio" aria-checked={option.value === value} onClick={() => onChange(option.value)}>{option.label}</button>)}
  </div>;
}

/* ---- Feedback ----------------------------------------------------------- */

const alertIcon = { info: Info, success: CheckCircle2, warning: AlertTriangle, danger: OctagonAlert } as const;

export function Alert({ tone = "info", title, children, action, className }: { tone?: keyof typeof alertIcon; title?: string; children?: ReactNode; action?: ReactNode; className?: string }) {
  const Icon = alertIcon[tone];
  return <div role={tone === "danger" || tone === "warning" ? "alert" : "status"} className={classes("ui-alert", `ui-alert-${tone}`, className)}>
    <Icon size={18} aria-hidden="true" />
    <div>{title && <strong>{title}</strong>}{children && <p>{children}</p>}</div>
    {action}
  </div>;
}

export type ToastItem = { id: number; tone?: "success" | "danger" | "info"; message: string };

/** The live region that holds toasts. The caller owns the list and the timers. */
export function ToastRegion({ toasts, onDismiss }: { toasts: ToastItem[]; onDismiss: (id: number) => void }) {
  return <div className="ui-toasts" role="region" aria-label="Notifications" aria-live="polite">
    {toasts.map((toast) => <div key={toast.id} className={classes("ui-toast", `ui-toast-${toast.tone ?? "info"}`)}><span>{toast.message}</span><IconButton label="Dismiss" onClick={() => onDismiss(toast.id)}><X size={14} /></IconButton></div>)}
  </div>;
}

export function Progress({ value, max = 100, label, tone = "accent" }: { value: number; max?: number; label: string; tone?: "accent" | "danger" | "warning" }) {
  return <div className="ui-progress" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={max} aria-valuenow={value} data-tone={tone}><span style={{ width: `${Math.min(100, Math.max(0, value / max * 100))}%` }} /></div>;
}

/* ---- Display ------------------------------------------------------------ */

export function Avatar({ name, size = 32 }: { name: string; size?: number }) {
  const initials = name.split(/[\s@.]+/).filter(Boolean).slice(0, 2).map((part) => part[0].toUpperCase()).join("");
  return <span className="ui-avatar" style={{ width: size, height: size, fontSize: Math.round(size * 0.38) }} role="img" aria-label={name}>{initials}</span>;
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="ui-kbd">{children}</kbd>;
}

export function Breadcrumb({ items }: { items: { label: string; href?: string }[] }) {
  return <nav className="ui-breadcrumb" aria-label="Breadcrumb"><ol>{items.map((item, index) => <li key={item.label}>{index > 0 && <ChevronRight size={13} aria-hidden="true" />}{item.href && index < items.length - 1 ? <a href={item.href}>{item.label}</a> : <span aria-current={index === items.length - 1 ? "page" : undefined}>{item.label}</span>}</li>)}</ol></nav>;
}

export function Disclosure({ summary, children, defaultOpen }: { summary: ReactNode; children: ReactNode; defaultOpen?: boolean }) {
  return <details className="ui-disclosure" open={defaultOpen}><summary>{summary}</summary><div>{children}</div></details>;
}

export type StatProps = { label: string; value: string; /** Change against the previous period, as +12%. */ delta?: string; trend?: "up" | "down" | "flat"; /** Whether up is good. A refusal count going up is not. */ upIsGood?: boolean; spark?: ReactNode };

/** The headline numbers of one view. Four fit a row; more wrap. */
export function StatStrip({ items }: { items: StatProps[] }) {
  return <dl className="ui-stats">{items.map((item) => {
    const good = item.trend === "flat" || item.trend === undefined ? undefined : (item.trend === "up") === (item.upIsGood ?? true);
    return <div key={item.label}><dt>{item.label}</dt><dd>{item.value}</dd>
      {item.delta && <small data-good={good === undefined ? undefined : String(good)}>{item.trend === "up" ? "▲ " : item.trend === "down" ? "▼ " : ""}{item.delta}{item.trend !== "flat" && <span> vs previous</span>}</small>}
      {item.spark}</div>;
  })}</dl>;
}

export function Table({ caption, children, className }: { caption?: string; children: ReactNode; className?: string }) {
  return <div className="ui-table-scroll"><table className={classes("ui-table", className)}>{caption && <caption>{caption}</caption>}{children}</table></div>;
}

/* ---- Overlays ----------------------------------------------------------- */

/** A hint on hover and focus. It never carries the only copy of anything. */
export function Tooltip({ text, children }: { text: string; children: ReactElement<{ "aria-describedby"?: string }> }) {
  const id = useId();
  return <span className="ui-tooltip">{cloneElement(children, { "aria-describedby": id })}<span role="tooltip" id={id}>{text}</span></span>;
}

export type MenuItem = { label: string; icon?: ReactNode; onSelect: () => void; danger?: boolean; disabled?: boolean };

/** A button that opens a short list of actions. Arrow keys move, Escape returns to the button. */
export function Menu({ label, trigger, items, align = "end" }: { label: string; trigger: ReactNode; items: MenuItem[]; align?: "start" | "end" }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener("pointerdown", close);
    root.current?.querySelector<HTMLElement>("[role=menuitem]:not(:disabled)")?.focus();
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);
  const key = (event: KeyboardEvent) => {
    const nodes = [...root.current!.querySelectorAll<HTMLElement>("[role=menuitem]:not(:disabled)")];
    const index = nodes.indexOf(document.activeElement as HTMLElement);
    if (event.key === "Escape") { setOpen(false); button.current?.focus(); }
    else if (event.key === "ArrowDown") nodes[(index + 1) % nodes.length]?.focus();
    else if (event.key === "ArrowUp") nodes[(index - 1 + nodes.length) % nodes.length]?.focus();
    else if (event.key === "Home") nodes[0]?.focus();
    else if (event.key === "End") nodes[nodes.length - 1]?.focus();
    else if (event.key === "Tab") setOpen(false);
    else return;
    event.preventDefault();
  };
  return <div className="ui-menu" ref={root} onKeyDown={open ? key : undefined}>
    <Button ref={button} aria-haspopup="menu" aria-expanded={open} aria-label={label} onClick={() => setOpen(!open)}>{trigger}</Button>
    {open && <div role="menu" aria-label={label} className="ui-menu-list" data-align={align}>
      {items.map((item) => <button key={item.label} type="button" role="menuitem" disabled={item.disabled} className={item.danger ? "is-danger" : undefined} onClick={() => { setOpen(false); button.current?.focus(); item.onSelect(); }}>{item.icon}{item.label}</button>)}
    </div>}
  </div>;
}

/* ---- App patterns ------------------------------------------------------- */

/** The column a small app lives in. */
export function Page({ children, className }: { children: ReactNode; className?: string }) {
  return <main className={classes("ui-page", className)}>{children}</main>;
}

export function PageHeader({ eyebrow, title, actions }: { eyebrow?: string; title: string; actions?: ReactNode }) {
  return <header className="ui-page-header"><div>{eyebrow && <p className="ui-eyebrow">{eyebrow}</p>}<h1 className="ui-title">{title}</h1></div>{actions}</header>;
}

export function Lede({ children }: { children: ReactNode }) {
  return <p className="ui-lede">{children}</p>;
}

/** Who is using the app, with the one button that changes it. */
export function Identity({ name, children }: { name?: string; children?: ReactNode }) {
  return <div className="ui-identity">{name && <><Avatar name={name} size={26} /><span>{name}</span></>}{children}</div>;
}

export function SectionHead({ label, meta }: { label: string; meta?: ReactNode }) {
  return <div className="ui-section-head"><span>{label}</span>{meta !== undefined && <span>{meta}</span>}</div>;
}

export function List({ children }: { children: ReactNode }) {
  return <div className="ui-list" role="list">{children}</div>;
}

/** One row: an optional leading control, a title with a quiet line under it, and what can be done to it. */
export function ListItem({ title, detail, lead, end, done }: { title: ReactNode; detail?: ReactNode; lead?: ReactNode; end?: ReactNode; done?: boolean }) {
  return <article className="ui-list-item" role="listitem" data-done={done || undefined}>
    {lead && <span className="ui-list-lead">{lead}</span>}
    <div className="ui-list-main"><p>{title}</p>{detail && <small>{detail}</small>}</div>
    {end && <div className="ui-list-end">{end}</div>}
  </article>;
}

/** One line that adds a row: a field and its button, joined. */
export function Composer({ label, placeholder, action, value, onChange, onSubmit }: { label: string; placeholder?: string; action: ReactNode; value: string; onChange: (value: string) => void; onSubmit: () => void }) {
  return <form className="ui-composer" onSubmit={(event) => { event.preventDefault(); onSubmit(); }}>
    <input aria-label={label} placeholder={placeholder} value={value} autoComplete="off" onChange={(event) => onChange(event.target.value)} />
    <Button type="submit" variant="primary">{action}</Button>
  </form>;
}

export function Tiles({ children, small }: { children: ReactNode; small?: boolean }) {
  return <div className={classes("ui-tiles", small && "ui-tiles-small")}>{children}</div>;
}

export function Tile({ src, alt, caption, end, children }: { src?: string; alt?: string; caption?: ReactNode; end?: ReactNode; children?: ReactNode }) {
  return <figure className="ui-tile">{src ? <img src={src} alt={alt ?? ""} /> : <div className="ui-tile-media">{children}</div>}{(caption || end) && <figcaption><span>{caption}</span>{end}</figcaption>}</figure>;
}

/** Marks one row done or not done. The label names the row for a screen reader. */
export function Tick({ pressed, onToggle, label = "Toggle task" }: { pressed: boolean; onToggle: () => void; label?: string }) {
  return <button type="button" className="ui-tick" aria-pressed={pressed} aria-label={label} onClick={onToggle} />;
}
