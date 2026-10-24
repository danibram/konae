import { templateHelpers, templateStyles } from "./shared";
import type { TemplateSource } from "./types";

const schema = `-- What members write during the day. The nightly digest stamps digested_at
-- on the entries it mailed, so an entry goes out once.
CREATE TABLE IF NOT EXISTS entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  body TEXT NOT NULL CHECK (length(trim(body)) BETWEEN 1 AND 500),
  created_at INTEGER NOT NULL,
  digested_at INTEGER
);
`;

const model = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const entries = sqliteTable("entries", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  userId: text("user_id").notNull(),
  body: text("body").notNull(),
  createdAt: integer("created_at").notNull(),
  digestedAt: integer("digested_at"),
});
`;

const actions = {
  version: 1,
  actions: {
    "entries.list": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT id, body, created_at, digested_at FROM entries WHERE user_id = ?1 ORDER BY id DESC LIMIT 50",
      bindings: [{ source: "actor", field: "id" }],
    },
    "entries.add": {
      kind: "mutation",
      access: "authenticated",
      statement: "INSERT INTO entries (user_id, body, created_at) VALUES (?1, ?2, ?3)",
      bindings: [
        { source: "actor", field: "id" },
        { source: "input", key: "body", type: "string", maxLength: 500 },
        { source: "kernel", field: "now" },
      ],
    },
    // Every entry written before today's UTC midnight and not yet mailed. The
    // Kernel sends each author one email listing only the entries they wrote.
    "digest.send": {
      kind: "mutation",
      access: "authenticated",
      schedule: { daily: "07:00" },
      statement: "UPDATE entries SET digested_at = ?1 WHERE digested_at IS NULL AND created_at < CAST(?1 / 86400000 AS INTEGER) * 86400000",
      bindings: [{ source: "kernel", field: "now" }],
      notify: {
        to: { column: "user_id" },
        changes: ["body"],
        subject: "Your log: {changes.count} entries",
        body: "What you wrote before {kernel.now}:\n{changes}",
      },
    },
  },
};

const html = `
<title>What happened</title>
<main class="app-shell" data-purpose="A daily log. Each member writes entries; every morning at 07:00 UTC each member who wrote gets one email listing their entries.">
  <header>
    <div>
      <p class="eyebrow">Daily log</p>
      <h1>What happened</h1>
    </div>
    <div class="identity"><span id="identity"></span><button id="sign-in" class="quiet" hidden>Sign in</button></div>
  </header>

  <p class="intro">Every morning at 07:00 UTC the app emails each member the entries they wrote the day before. Nobody presses a button for it.</p>
  <p id="status" class="status" role="status" aria-live="polite"></p>

  <section id="log" hidden>
    <form id="entry-form" class="card">
      <label>New entry<input id="entry-body" maxlength="500" autocomplete="off" required></label>
      <button class="primary" type="submit">Add</button>
    </form>
    <div class="section-label"><span>Your entries</span></div>
    <div id="entries" class="rows"></div>
  </section>
</main>`;

const script = `${templateHelpers}
async function load() {
  const entries = await miniapp.actions.call('entries.list');
  $('#entries').innerHTML = entries.length ? entries.map(entry =>
    '<article class="row"><div><p>' + escapeHtml(entry.body) + '</p><small>' + when(entry.created_at) + '</small></div><span>'
      + (entry.digested_at ? 'mailed ' + when(entry.digested_at) : "in tomorrow's email") + '</span></article>'
  ).join('') : '<p class="empty">No entries yet. Tomorrow at 07:00 UTC you get an email with what you write today.</p>';
}

async function start() {
  const session = await miniapp.auth.session();
  const signedIn = session.role !== 'anonymous';
  $('#identity').textContent = signedIn ? session.name : '';
  $('#sign-in').hidden = signedIn;
  $('#log').hidden = !signedIn;
  if (signedIn) await load();
}

$('#sign-in').addEventListener('click', async () => { await miniapp.auth.signIn(); await start(); });

$('#entry-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const body = $('#entry-body').value.trim();
  if (!body) return;
  try {
    statusLine.textContent = '';
    await miniapp.actions.call('entries.add', { body });
    $('#entry-body').value = '';
    await load();
  } catch (error) { showError(error); }
});

try { await start(); } catch (error) { showError(error); }
`;

export const digestTemplate: TemplateSource = {
  id: "digest",
  name: "Nightly digest",
  sentence: "Members write entries; each morning every member gets an email listing what they wrote the day before.",
  model,
  files: {
    "index.html": html,
    "styles.css": templateStyles,
    "app.js": script,
    "schema.sql": schema,
    "actions.json": `${JSON.stringify(actions, null, 2)}\n`,
  },
};
