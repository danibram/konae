# Littlebox

Littlebox is the guardrail that lets a person who does not write code, and a person who does, make the same small app and take it to another machine with its accounts, its data and its rules unchanged.

That sentence is the yardstick. A review that scores polish without it can call the workshop finished while a person still cannot tell what the checks refused, or cannot take the app with them.

## Jobs

1. A person who does not write code describes an app and gets one that runs.
2. A person who writes code edits the same app's files.
3. Both of them hit the same checks. A query that reads rows names a LIMIT, in the editor and on the Kernel that leaves with the app.
4. Either person can take the app to another machine with its accounts, its data and its rules.
5. Who may open the app, and who may read a row, stay decisions of the app. The host does not replace them.

## Where the work lives

- `src/` is the editor. `src/components/ProjectDashboard.tsx` is the home, `src/components/AgentPanel.tsx` is the conversation, and `src/App.tsx` is the file editor.
- `src/agent/system-prompt.ts` is what the build agent is told. `src/agent/gate.ts` is the Validation Gate. `src/lib/query-bound.ts` is the row limit both doors share.
- `src/kernel/actions.ts` parses `actions.json` for the editor. `kernel-rs/src/main.rs` parses it again for every deployment, including an export.
- `CONTEXT.md` holds the language. `docs/adr/` holds the decisions. `docs/architecture.md` is the shape. `examples/product-fit/` is the set of apps used to see what the Kernel can publish.

## Critic

Judge the running editor at `http://localhost:4173` against the sentence at the top and the five jobs. The home is `/`. A project opens at `/app/<id>`. The starter on a fresh browser is `/app/app_tiny_tasks`. `/design-system` is the component gallery and `/platform-access` is owner sign-in. The theme follows the system. Do not clear storage.
