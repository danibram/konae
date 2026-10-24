import { templateHelpers, templateStyles } from "./shared";
import type { TemplateSource } from "./types";

const schema = `-- Written only by the signed route. A redelivery of the same event changes nothing.
CREATE TABLE IF NOT EXISTS events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  external_id TEXT NOT NULL,
  title TEXT NOT NULL,
  repo TEXT NOT NULL DEFAULT '',
  received_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS events_external_id_unique ON events(external_id);
`;

const model = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const events = sqliteTable("events", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  externalId: text("external_id").notNull().unique(),
  title: text("title").notNull(),
  repo: text("repo").notNull().default(""),
  receivedAt: integer("received_at").notNull(),
});
`;

const actions = {
  version: 1,
  actions: {
    "github.receive": {
      kind: "mutation",
      access: "anonymous",
      http: {
        method: "POST",
        path: "/hooks/github",
        signature: { provider: "github", secret: "github_webhook" },
        fields: { id: "$.issue.node_id", title: "$.issue.title", repo: "$.repository.full_name" },
      },
      statement: "INSERT INTO events (external_id, title, repo, received_at) VALUES (?1, ?2, COALESCE(?3, ''), ?4) ON CONFLICT (external_id) DO NOTHING",
      bindings: [
        { source: "input", key: "id", type: "string", maxLength: 100 },
        { source: "input", key: "title", type: "string", maxLength: 200 },
        { source: "input", key: "repo", type: "string", maxLength: 200, nullable: true },
        { source: "kernel", field: "now" },
      ],
    },
    "events.list": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT id, external_id, title, repo, received_at FROM events WHERE ?1 = 'owner' ORDER BY id DESC LIMIT 100",
      bindings: [{ source: "actor", field: "role" }],
    },
  },
};

const html = `
<title>Issue feed</title>
<main class="app-shell" data-purpose="A webhook receiver. GitHub sends signed issue events to /hooks/github; the owner reads them here.">
  <header>
    <div>
      <p class="eyebrow">Webhook</p>
      <h1>Issue feed</h1>
    </div>
    <div class="identity"><span id="identity"></span><button id="sign-in" class="quiet">Owner sign in</button></div>
  </header>

  <p class="intro">Point a GitHub webhook for issue events at <code>/hooks/github</code> on this app, with the secret you save as <code>github_webhook</code> under Integrations. The Kernel checks every signature and drops a redelivery.</p>
  <p id="status" class="status" role="status" aria-live="polite"></p>

  <section id="feed" hidden>
    <div class="section-label"><span>Events</span><span>Only the owner sees these</span></div>
    <div id="events" class="rows"></div>
  </section>
</main>`;

const script = `${templateHelpers}
async function start() {
  const session = await miniapp.auth.session();
  $('#identity').textContent = session.role === 'anonymous' ? '' : session.name;
  $('#sign-in').hidden = session.role !== 'anonymous';
  $('#feed').hidden = session.role !== 'owner';
  if (session.role !== 'owner') return;
  const events = await miniapp.actions.call('events.list');
  $('#events').innerHTML = events.length ? events.map(event =>
    '<article class="row"><div><p>' + escapeHtml(event.title) + '</p><small>' + escapeHtml(event.repo || 'GitHub') + ' · ' + when(event.received_at) + '</small></div><span></span></article>'
  ).join('') : '<p class="empty">No events yet. They arrive once the webhook is set up.</p>';
}

$('#sign-in').addEventListener('click', async () => { await miniapp.auth.signIn(); await start(); });

try { await start(); } catch (error) { showError(error); }
`;

export const webhookTemplate: TemplateSource = {
  id: "webhook",
  name: "Webhook receiver",
  sentence: "Another service sends signed events to the app; the owner reads them here.",
  visibility: "private",
  model,
  files: {
    "index.html": html,
    "styles.css": templateStyles,
    "app.js": script,
    "schema.sql": schema,
    "actions.json": `${JSON.stringify(actions, null, 2)}\n`,
  },
};
