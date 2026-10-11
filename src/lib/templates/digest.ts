import { templateHelpers, templateStyles } from "./shared";
import type { TemplateSource } from "./types";

const schema = `-- What members write during the day.
CREATE TABLE IF NOT EXISTS entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  body TEXT NOT NULL CHECK (length(trim(body)) BETWEEN 1 AND 500),
  created_at INTEGER NOT NULL
);

-- One row per UTC day that had entries, written only by the nightly digest.
CREATE TABLE IF NOT EXISTS digests (
  day TEXT PRIMARY KEY NOT NULL,
  entries INTEGER NOT NULL,
  sent_at INTEGER NOT NULL
);
`;

const model = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const entries = sqliteTable("entries", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  userId: text("user_id").notNull(),
  body: text("body").notNull(),
  createdAt: integer("created_at").notNull(),
});

export const digests = sqliteTable("digests", {
  day: text("day").primaryKey(),
  entries: integer("entries").notNull(),
  sentAt: integer("sent_at").notNull(),
});
`;

const actions = {
  version: 1,
  actions: {
    "entries.list": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT id, body, created_at FROM entries ORDER BY id DESC LIMIT 50",
      bindings: [],
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
    "digests.list": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT day, entries, sent_at FROM digests ORDER BY day DESC LIMIT 14",
      bindings: [],
    },
    "digest.send": {
      kind: "mutation",
      access: "authenticated",
      schedule: { daily: "07:00" },
      statement: "INSERT INTO digests (day, entries, sent_at) SELECT day, n, ?1 FROM (SELECT date(?1 / 1000, 'unixepoch', '-1 day') AS day, count(*) AS n FROM entries WHERE created_at >= (CAST(?1 / 86400000 AS INTEGER) - 1) * 86400000 AND created_at < CAST(?1 / 86400000 AS INTEGER) * 86400000) WHERE n > 0 ON CONFLICT (day) DO NOTHING",
      bindings: [{ source: "kernel", field: "now" }],
      notify: {
        to: "owner",
        subject: "Yesterday in the log",
        body: "New entries arrived in the log yesterday.\nOpen the app to read them.",
      },
    },
  },
};

const html = `
<main class="app-shell" data-purpose="A shared log. Members write entries; every morning at 07:00 UTC the owner gets one email when yesterday had any.">
  <header>
    <div>
      <p class="eyebrow">Daily log</p>
      <h1>What happened</h1>
    </div>
    <div class="identity"><span id="identity"></span><button id="sign-in" class="quiet" hidden>Sign in</button></div>
  </header>

  <p class="intro">Every morning at 07:00 UTC the app emails its owner when the day before had entries. Nobody presses a button for it.</p>
  <p id="status" class="status" role="status" aria-live="polite"></p>

  <section id="log" hidden>
    <form id="entry-form" class="card">
      <label>New entry<input id="entry-body" maxlength="500" autocomplete="off" required></label>
      <button class="primary" type="submit">Add</button>
    </form>
    <div class="section-label"><span>Entries</span></div>
    <div id="entries" class="rows"></div>
    <div class="section-label"><span>Digests sent</span></div>
    <div id="digests" class="rows"></div>
  </section>
</main>`;

const script = `${templateHelpers}
async function load() {
  const [entries, digests] = await Promise.all([miniapp.actions.call('entries.list'), miniapp.actions.call('digests.list')]);
  $('#entries').innerHTML = entries.length ? entries.map(entry =>
    '<article class="row"><div><p>' + escapeHtml(entry.body) + '</p><small>' + when(entry.created_at) + '</small></div><span></span></article>'
  ).join('') : '<p class="empty">No entries yet.</p>';
  $('#digests').innerHTML = digests.length ? digests.map(digest =>
    '<article class="row"><div><p>' + escapeHtml(digest.day) + '</p><small>' + digest.entries + ' entries · sent ' + when(digest.sent_at) + '</small></div><span></span></article>'
  ).join('') : '<p class="empty">The first digest goes out the morning after the first entry.</p>';
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
  sentence: "Members write entries; each morning the owner gets one email about the day before.",
  model,
  files: {
    "index.html": html,
    "styles.css": templateStyles,
    "app.js": script,
    "schema.sql": schema,
    "actions.json": `${JSON.stringify(actions, null, 2)}\n`,
  },
};
