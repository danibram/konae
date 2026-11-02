# Make your first app from the editor

Describe an app in plain words, apply the change the Validation Gate lets through, and open the published Deployment URL.

## Before you start

The editor runs at `http://localhost:4173` after `bun run dev:local`, at `http://app.localhost:8080` under Docker Compose, and at `https://app.<domain>` on an installation with `KONAE_DOMAIN` set. The steps are the same in each.

You need to be signed in as the owner. Open **Platform access** in the editor to create an owner account or sign in. The editor shows **Sign in to deploy** on the deploy dialog until you do, and brings you back to the app afterwards.

## Steps

1. On the project list, create a project. The dialog is titled **Name this app**. Give it a name and pick a starting point under **Start with**. Each starting point runs as it is.
2. In the conversation panel, press **Connect a model** and paste an Anthropic, OpenAI or OpenRouter key. The key stays in this browser. Then describe the app in one sentence. The prompt box reads "Describe the app."
3. Read the proposed change. The files stay as they are until you press **Apply**.
4. Press **Apply**. The Validation Gate, the editor's check, reads the change before it reaches the project. A refusal names what to change. The Gate refuses the change when:
   - the schema is not valid;
   - a Server Action's SQL does not run against the schema;
   - a query reads rows without a LIMIT.
5. Try the app in the preview. Ask for changes in the same panel and apply each one.
6. Press **Deploy**. The dialog is headed **Deploy** followed by the app's name. It shows "Every query names its LIMIT. It can leave." when nothing blocks the release.
7. Choose who can open the app, **Public** (anyone can open it) or **Private** (people with an app account). Then choose who can create an app account. The dialog preselects Open for a public app and Invitation only for a private one.
8. Confirm with the button that names your choice, **Deploy public** or **Deploy private**. The dialog then shows **Published publicly** or **Published privately** with the Deployment URL. When a republish removes a table or a column from the published database, or changes a column's type, a dialog titled **This publish deletes data** lists what goes first. **Keep the data** publishes nothing, and **Publish and delete** applies it.
9. Open the Deployment URL and use the app once. That calls a Server Action on the Kernel.

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

The Gate checks the schema and `actions.json`. A page that names a missing file fails at build or release, not at the Gate, so check every path you write.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Invite people](./invite-people.md)
- [Edit the same app's files with the CLI](./edit-files-with-the-cli.md)
- [The Validation Gate and the Kernel](../gate-and-kernel.md)
- [Preview and Deployment differences](../divergences.md)
