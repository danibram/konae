# Make your first app from the editor

Describe an app in plain words, apply the change the Validation Gate lets through, and open the published Deployment URL.

## Before you start

The editor runs at `http://localhost:4173` after `bun run dev:local`, at `http://app.localhost:8080` under Docker Compose, and at `https://app.<domain>` on an installation with `KONAE_DOMAIN` set. The steps are the same in each.

The editor opens on the owner sign-in page. Sign in, or press **Create owner account**. The first owner of an installation gives the setup key, which a local install fills in, and every later owner gives an invitation code from the operator. A deploy dialog opened while signed out shows **Sign in to deploy** and brings you back to the app afterwards.

The build agent needs an API key from Anthropic, OpenAI or OpenRouter, and the provider bills the key's account. Step 2 says where to create one.

## Steps

1. On the project list, press **New project**. The dialog asks **What should it do?** first. Write a sentence, open **Start from a template instead** and pick one, or both; each template runs as it is. A sentence with no template starts as a private list titled with the project's name, with your sentence at the top and on its card. The sentence waits in **Describe** until you send it, even if you leave the app or reload. The name is optional and comes from your first words.
2. In the conversation panel, press **Connect a model** and paste an Anthropic, OpenAI or OpenRouter key. The dialog links to the provider's page where you create one. The key stays in this browser, and the provider bills it for what Konae sends. Save closes the dialog, and the sentence from New project waits in the prompt box. Press **Send**, or write a sentence there first when you started from a template.
3. Read the proposed change. The files stay as they are until you press **Apply**.
4. Read the line above **Apply**. The Validation Gate, the editor's check, has read the change before it can reach the project, and shows "Validated. Schema, preview rows and actions hold." when it passes. A refusal names what to change instead. The Gate refuses the change when:
   - the schema is not valid;
   - a Server Action's SQL does not run against the schema;
   - a query reads rows without a LIMIT;
   - every page is a whole document that loads no script, so the app's entrypoint, `app.js`, `src/main.ts` or `src/main.tsx`, would never run.
5. Press **Apply**, then try the app in the preview. Ask for changes in the same panel and apply each one.
6. Press **Deploy** in the top bar. Once the app is published the same button reads **Deploy changes**. The dialog is headed **Deploy** followed by the app's name, and lists the address, who can open the app and what people can do. When something blocks the release it shows only the refusal, with a button to the file or the setting that clears it, and its Deploy button stays off.
7. Choose who can open the app, **Public** (anyone can open it) or **Private** (people with an app account). Then choose who can create an app account. The dialog preselects Open for a public app and Invitation only for a private one. Later, who can open the app changes in Access > Rules and who can create an account in Access > Users.
8. Confirm with the button that names your choice, **Deploy public** or **Deploy private**. The dialog then shows the Deployment URL and **Open the app**. When a republish removes a table or a column from the published database, or changes a column's type, a dialog titled **This publish deletes data** lists what goes first. **Keep the data** publishes nothing, and **Publish and delete** applies it.
9. Open the Deployment URL and use the app once. That calls a Server Action on the Kernel. A private app asks for an app account first. When the app has none, the dialog says so and offers **Invite yourself**, which invites your owner email in one press. **Invite people** opens Access > Users with the cursor in the invite field. On `bun run dev:local` nothing sends email. Open `data/kernel-local/mailbox.jsonl` in the Konae checkout, find the last line whose `to` is your address and whose `purpose` is `invitation`, and open the URL in its `link` field.

## Check it worked

Observe the result through the same door you wrote through, before you tell anyone it worked.

- On the project list, the card moves under **Published** and shows the Deployment's address once a Deployment exists. Open the address and use one feature that reads or writes a row.
- In the editor's top bar, **Changes not deployed** appears beside the app's state while Files, a rule in Access or who can open the app differ from what the published app runs. A deploy clears it.
- Over MCP, the read-back for `release_apply` is `project_inspect`. It returns the live Deployment and its URL.
- To see what the Gate's change did to the files, the read-back for `project_sources_save` is `project_sources_read`.

## Data and republishing

Data opens on the published app's live rows once the app is published, and on the test rows in this browser before that. Use it to switch between the two, browse and edit rows, run SQL, create backups and restore them. `schema.sql` is the only model. A publish changes the published database to match it, and asks before it deletes a table or a column. The preview keeps its own test rows and does not change a table it already has, so try a changed table on the published app or with `konae dev`. Publishing the same project keeps its URL and database, and the republished app stops serving files that the new artifact no longer contains.

Access > Rules holds who can open the app, who can read uploaded files and who can run each action, in the words the Deploy dialog uses. A change there runs in the preview at once and reaches the published app with the next deploy. Its **Preview as** lets you try the app as Guest, Alice and Bob in the preview. A published app uses its own accounts.

## What the preview cannot show

The editor preview is not a Deployment. It applies no Content Security Policy, so inline handlers and fetches to other origins still run there, and a published app refuses them. Try these on `konae dev` or on the published Deployment, because the preview has none of them.

- HTTP routes, an `http` block on an anonymous action
- the schedule timer
- notice email, from `notify`
- calls to other services, from `kind: call`
- the Usage store
- Google sign-in
- the installation's file limit, which the preview fixes at 2 MiB, where `konae dev` and a Deployment refuse at `KONAE_FILE_MAX_BYTES`, 2 MiB unless raised

The Gate checks the schema, `actions.json` and that a page runs the app's entrypoint. A page that names a missing file fails at build or release, not at the Gate, so check every path you write.

## Take it with you

**Export**, under **More** in the editor's top bar, downloads the app folder as a tar, with the published app's database, accounts and files. [Take the app elsewhere](./take-the-app-elsewhere.md) runs it on another machine. App settings, under **More**, holds the app's addresses, its contact email, maintenance and an import of an exported folder.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Invite people](./invite-people.md)
- [Edit the same app's files with the CLI](./edit-files-with-the-cli.md)
- [The Validation Gate and the Kernel](../gate-and-kernel.md)
- [Preview and Deployment differences](../divergences.md)
