# Make your first app from the editor

Describe an app in plain words, apply the change the Validation Gate lets through, and open the published Deployment URL.

## Before you start

The editor runs at `http://localhost:4173` after `bun run dev:local`, at `http://app.localhost:8080` under Docker Compose, and at `https://app.<domain>` on an installation with `KONAE_DOMAIN` set. The steps are the same in each.

The editor opens on the owner sign-in page. Sign in, or press **Create owner account**. The first owner of an installation gives the setup key, which a local install fills in, and every later owner gives an invitation code from the operator. A deploy dialog opened while signed out shows **Sign in to deploy** and brings you back to the app afterwards.

## Steps

1. On the project list, create a project. The dialog is titled **Name this app**. Give it a name and pick a starting point under **Start with**. Each starting point runs as it is.
2. In the conversation panel, press **Connect a model** and paste an Anthropic, OpenAI or OpenRouter key. The key stays in this browser. Then describe the app in one sentence. The prompt box reads "Describe the app."
3. Read the proposed change. The files stay as they are until you press **Apply**.
4. Read the line above **Apply**. The Validation Gate, the editor's check, has read the change before it can reach the project, and shows "Validated. Schema, preview rows and actions hold." when it passes. A refusal names what to change instead. The Gate refuses the change when:
   - the schema is not valid;
   - a Server Action's SQL does not run against the schema;
   - a query reads rows without a LIMIT;
   - every page is a whole document that loads no script, so the app's entrypoint, `app.js`, `src/main.ts` or `src/main.tsx`, would never run.
5. Press **Apply**, then try the app in the preview. Ask for changes in the same panel and apply each one.
6. Press **Deploy**. The dialog is headed **Deploy** followed by the app's name. It shows "Every query names its LIMIT. It can leave." when nothing blocks the release.
7. Choose who can open the app, **Public** (anyone can open it) or **Private** (people with an app account). Then choose who can create an app account. The dialog preselects Open for a public app and Invitation only for a private one, and you change it later in Auth > Users.
8. Confirm with the button that names your choice, **Deploy public** or **Deploy private**. The dialog then shows **Published publicly** or **Published privately** with the Deployment URL. When a republish removes a table or a column from the published database, or changes a column's type, a dialog titled **This publish deletes data** lists what goes first. **Keep the data** publishes nothing, and **Publish and delete** applies it.
9. Open the Deployment URL and use the app once. That calls a Server Action on the Kernel. A private app asks for an app account first. Press **Invite people** on the dialog, or open Auth > Users, and invite yourself. On `bun run dev:local` no email leaves the machine. Right after you send, the Users view shows the invitation link once, and the message also lands in `data/kernel-local/mailbox.jsonl`.

## Check it worked

Observe the result through the same door you wrote through, before you tell anyone it worked.

- In the editor, the project card shows **Open published app** once a Deployment exists. Open it and use one feature that reads or writes a row.
- Over MCP, the read-back for `release_apply` is `project_inspect`. It returns the live Deployment and its URL.
- To see what the Gate's change did to the files, the read-back for `project_sources_save` is `project_sources_read`.

## Data and republishing

Use Data to choose the local or deployed database, browse and edit rows, run SQL, create backups and restore them. `schema.sql` is the only model. A publish changes the published database to match it, and asks before it deletes a table or a column. The preview keeps its own test rows and does not change a table it already has, so try a changed table on the published app or with `konae dev`. Publishing the same project keeps its URL and database, and the republished app stops serving files that the new artifact no longer contains.

The Auth panel lets you try the app as Guest, Alice and Bob in the preview. A published app uses its own accounts.

## What the preview cannot show

The editor preview is not a Deployment. It applies no Content Security Policy, so inline handlers and fetches to other origins still run there, and a published app refuses them. It has no HTTP routes, no schedule timer, no notice email, no Usage store and no call outbox. Try those on `konae dev` or on the published Deployment.

The Gate checks the schema, `actions.json` and that a page runs the app's entrypoint. A page that names a missing file fails at build or release, not at the Gate, so check every path you write.

## Take it with you

**Export** in the editor's header downloads the app folder as a tar, with the published app's database, accounts and files. [Take the app elsewhere](./take-the-app-elsewhere.md) runs it on another machine. App settings, under **More**, holds the app's addresses, its contact email, maintenance and an import of an exported folder.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Invite people](./invite-people.md)
- [Edit the same app's files with the CLI](./edit-files-with-the-cli.md)
- [The Validation Gate and the Kernel](../gate-and-kernel.md)
- [Preview and Deployment differences](../divergences.md)
