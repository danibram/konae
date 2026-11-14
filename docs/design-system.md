# Konae design system

The editor and the apps it makes share one set of tokens and components. The default variant has square corners, 2px ink strokes, Bricolage Grotesque and one lime accent. Primary buttons and dialogs keep hard offset shadows. Fields, secondary buttons and panel content stay flat. The clean variant swaps in system type with Inter, hairline borders, rounded corners and a black primary on the same tokens. The theme follows the system, light or dark.

## Foundations

`src/design-system.css` defines the tokens and the shared rules. `src/components/ui/variant-clean.css` overrides them inside any element with `data-ds="clean"`.

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
| Shadows | `--ds-shadow-sm`, `--ds-shadow`, `--ds-shadow-lg`, `--ds-shadow-dialog` |
| Spacing | `--ds-space-1`, `--ds-space-2`, `--ds-space-3`, `--ds-space-4`, `--ds-space-6`, `--ds-space-8` |

Body text is 14px. Controls and secondary text use 11 to 13px. Source code uses JetBrains Mono in the default variant. The spacing scale is 4, 8, 12, 16, 24 and 32px. Corners are 0 in the default variant and 4, 6 and 10px in the clean one.

Use the accent for primary actions and selection, and `--ds-blue` for the focus outline. Status colors describe an actual state. Keep decorative color off toolbars and table controls.

## Components

Import shared controls from `src/components/ui`. The ones a form or a dialog needs:

| Component | Options and behavior |
| --- | --- |
| `Button` | `variant` of `primary`, `secondary` (the default), `ghost` or `danger`; `size` of `sm`, `md` or `icon`; `loading` disables the control and sets `aria-busy`. Defaults to `type="button"`. |
| `IconButton` | Requires a `label` for its accessible name and tooltip. |
| `Input`, `Select`, `Textarea` | Native props, including `ref`, `disabled` and `aria-invalid`. |
| `Field` | Connects its label and optional help text to one child control. Keeps an explicit control ID and existing descriptions. |
| `Badge` | `tone` of `neutral`, `accent`, `success`, `warning` or `danger`. |
| `Card` | Shared border, background and corners. Takes normal div props. |
| `EmptyState` | Icon, title, description and optional action. |
| `Skeleton` | A quiet placeholder shape. Give it the width and height of the content that replaces it. Hidden from assistive technology. |
| `LoadingBlock` | One loading surface with `role="status"`, one label for screen readers and skeleton shapes that hold the layout. Use `rows` or your own children. Fades in after 250 ms. `sinceNavigation` counts that from the page load, for boot stages. |
| `Dialog` | Title, optional description and footer, `open`, `onClose`, optional `initialFocus`. Traps focus, handles Escape and restores focus when closed. |

The same module exports `Tabs`, `Switch`, `Checkbox`, `Radio`, `Segmented`, `Alert`, `ToastRegion`, `Progress`, `Avatar`, `Kbd`, `Breadcrumb`, `Disclosure`, `StatStrip`, `Table`, `Tooltip` and `Menu`, the charts in `charts.tsx` (`BarChart`, `BarList`, `LineChart`, `DonutChart`, `Sparkline`, `ChartLegend`), and the page patterns an app uses: `Page`, `PageHeader`, `Lede`, `Actions`, `Steps`, `Identity`, `SectionHead`, `List`, `ListItem`, `Composer`, `Tiles`, `Tile` and `Tick`. The gallery shows them.

```tsx
import { Button, Dialog, Field, Input } from "./components/ui";

<Dialog
  open={open}
  title="App settings"
  onClose={close}
  footer={<Button variant="primary" onClick={save}>Save changes</Button>}
>
  <div className="ui-dialog-body">
    <Field label="Name" help="Use a name you can recognize later.">
      <Input value={name} onChange={(event) => setName(event.target.value)} />
    </Field>
  </div>
</Dialog>
```

Use links for navigation and buttons for actions. Set `type="submit"` explicitly for form submission. Icon-only actions need `IconButton` or an accessible name. Give error fields `aria-invalid="true"` and explain the error in the field help.

## In published apps

A published app gets the design system as one stylesheet, with the components as `.ui-*` classes, such as `.ui-actions` for `Actions` and `.ui-list.ui-steps` for `Steps`. A page that stands on its own, such as the installation's home in `examples/home`, also uses `.ui-skip` for its skip link, `.ui-bar` for a top bar with the name and a few links, `.ui-page-wide` on the bar and the page when it needs room beside its text, and `.ui-pre`, a `figure` with an optional `figcaption` and a `pre`, for code or a message the reader should see whole.

`bun run ui:render` writes those stylesheets from `src/design-system.css` and `src/components/ui`: `src/lib/templates/app-ui.generated.ts`, with one stylesheet per variant for the templates, and `examples/home/styles.css` and `examples/docs-site/public/styles.css` in the default variant. It also copies `query-bound.ts` and `sql-tokens.ts` into `examples/home`. `src/lib/templates/app-ui.test.ts` fails when a written file drifts from its source, so run it after changing a token or a component's CSS.

## Where the editor uses it

The project list, the deploy dialog, the build agent's message box and the Data panel use the shared buttons, inputs, selects, badges, cards, dialogs, empty states and loading blocks. Access > Users and Access > Integrations use cards, fields, selects, badges, buttons and confirmation dialogs from the same set. Their layout rules live in `src/components/app-access.css` and use only `--ds-*` tokens, so the system theme carries light and dark.

The other panels keep their own layout selectors with token-based rules in `src/design-system.css`. New controls use the shared components.

## Loading

A screen renders its navigation, header and title on the first paint and keeps them. Whatever is still loading sits inside that frame as skeleton shapes the size of the content that replaces them. Use `LoadingBlock` for a surface and `Skeleton` for a single shape. Never swap one sentence for another, and never show a loader for work that ends in under 250 ms. `useLoadingHold` in `src/components/loading-hold.ts` keeps a skeleton that did show on screen for at least 400 ms. `heldLazy` in `src/components/held-lazy.tsx` applies the same delay and hold when a panel's module loads. It counts the hold from the fallback's CSS animation so a late first paint cannot shorten it. `BootShell` draws the page and editor chrome while the session check, the code chunk and the project load run.

## Gallery

Open `/design-system` to see the tokens, type, the components with their variants, loading and disabled states, validation and an interactive dialog, in either variant. The sidebar links to it while no owner is signed in.

The gallery creates no project, uploads no file and deletes no data. Its dialog and success-state demo change only local component state.
