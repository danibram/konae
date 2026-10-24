# Littlebox design system

The workspace uses a restrained interface. Neutral surfaces, a system sans serif and one indigo accent keep attention on projects and their tools.

## Foundations

`src/design-system.css` defines the tokens and shared visual rules.

| Role | Token |
| --- | --- |
| Page background | `--ds-canvas` |
| Cards and dialogs | `--ds-surface` |
| Secondary surfaces | `--ds-subtle` |
| Borders and separators | `--ds-border` |
| Primary and secondary text | `--ds-text`, `--ds-muted` |
| Primary action | `--ds-accent`, `--ds-accent-hover` |
| Selection | `--ds-accent-soft` |
| Status | `--ds-success`, `--ds-warning`, `--ds-danger` |
| Status backgrounds | `--ds-success-soft`, `--ds-warning-soft`, `--ds-danger-soft` |
| Type | `--ds-font`, `--ds-mono` |
| Corners | `--ds-radius-sm`, `--ds-radius`, `--ds-radius-lg` |
| Shadows | `--ds-shadow`, `--ds-shadow-dialog` |
| Spacing | `--ds-space-1`, `--ds-space-2`, `--ds-space-3`, `--ds-space-4`, `--ds-space-6`, `--ds-space-8` |

Body text is 14px. Controls use 13px, secondary text uses 12px and compact metadata uses 10 or 11px. Source code uses JetBrains Mono. The spacing scale is 4, 8, 12, 16, 24 and 32px. Corners are 4, 6 and 10px.

Use indigo for primary actions, selection and focus. Status colors describe an actual state. Keep decorative color off toolbars and table controls. Code editors keep their dark reading surface inside the light workspace.

## Components

Import shared controls from `src/components/ui`.

| Component | Options and behavior |
| --- | --- |
| `Button` | `primary`, `secondary`, `ghost`, `danger`; `sm`, `md`, `icon`; `loading` disables the control and sets `aria-busy`. Defaults to `type="button"`. |
| `IconButton` | Requires a `label` for its accessible name and tooltip. |
| `Input` | Native input props, including `ref`, `disabled` and `aria-invalid`. |
| `Select` | Native select props and children. |
| `Textarea` | Native textarea props and children. |
| `Field` | Connects its label and optional help text to one child control. Preserves an explicit control ID and existing descriptions. |
| `Badge` | `neutral`, `accent`, `success`, `warning`, `danger`. |
| `Card` | Shared border, background and corner radius. Accepts normal div props. |
| `EmptyState` | Icon, title, description and optional action. |
| `Skeleton` | A quiet placeholder shape. Give it the width and height of the content that replaces it. Hidden from assistive technology. |
| `LoadingBlock` | One loading surface. `role="status"` with one label for screen readers and skeleton shapes (`rows`, or your own children) that hold the layout. Fades in after 200 ms; `sinceNavigation` counts that from the page load for boot stages. |
| `Dialog` | Title, optional description and footer, `open`, `onClose`, optional `initialFocus`. Traps focus, handles Escape and restores focus when closed. |

```tsx
import { Button, Dialog, Field, Input } from "./components/ui";

<Dialog
  open={open}
  title="Project settings"
  onClose={close}
  footer={<Button variant="primary" onClick={save}>Save changes</Button>}
>
  <div className="ui-dialog-body">
    <Field label="Project name" help="Use a name you can recognize later.">
      <Input value={name} onChange={(event) => setName(event.target.value)} />
    </Field>
  </div>
</Dialog>
```

Use links for navigation and buttons for actions. Set `type="submit"` explicitly for form submission. Icon-only actions need `IconButton` or an accessible name. Give error fields `aria-invalid="true"` and explain the error in the field help.

## Adoption

The dashboard uses shared buttons, inputs, selects, badges and cards. Agent settings and publishing use the same dialog component. Agent messages use the shared textarea and connection button. Files and database tables use the shared empty state. Auth > Users and Auth > Integrations use cards, fields, selects, badges, buttons and confirmation dialogs from the same set; their layout rules live in `src/components/app-access.css` and use only `--ds-*` tokens, so the system theme carries light and dark.

Existing domain panels retain their layout selectors and use token-based compatibility rules in `src/design-system.css`. New controls should use the shared components. Keep SQL editing, table state and runtime permissions in their existing modules.

## Loading

A screen renders its chrome (navigation, header, title) on the first paint and keeps it. Whatever is still loading sits inside that frame as skeleton shapes with the size of the content that replaces them, so nothing moves. Use `LoadingBlock` for a surface and `Skeleton` for a single shape. Never swap one sentence for another, and never show a loader for work that ends in under 200 ms. `useLoadingHold` in `src/components/loading-hold.ts` keeps a skeleton that did show on screen for at least 300 ms. `BootShell` draws the page and editor chrome while the session check, the code chunk and the project load run.

## Gallery and verification

Open `/design-system` to inspect tokens, typography, button variants, loading and disabled states, status badges, fields, validation, cards, empty states and an interactive dialog. The desktop sidebar also links to the gallery.

The gallery does not create projects, upload files or delete data. Its dialog and success-state demo only change local component state.
