import { templateHelpers, templateStyles } from "./shared";
import type { TemplateSource } from "./types";

const schema = `-- One row per published image. The bytes live with the Kernel under file_id.
CREATE TABLE IF NOT EXISTS photos (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  file_id TEXT NOT NULL,
  title TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS photos_file_id_unique ON photos(file_id);
`;

const model = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const photos = sqliteTable("photos", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  userId: text("user_id").notNull(),
  fileId: text("file_id").notNull().unique(),
  title: text("title").notNull(),
  createdAt: integer("created_at").notNull(),
});
`;

const actions = {
  version: 1,
  actions: {
    "photos.list": {
      kind: "query",
      access: "anonymous",
      statement: "SELECT id, file_id, title, created_at FROM photos ORDER BY id DESC LIMIT 60",
      bindings: [],
    },
    "photos.mine": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT file_id FROM photos WHERE user_id = ?1 ORDER BY id DESC LIMIT 60",
      bindings: [{ source: "actor", field: "id" }],
    },
    "photos.add": {
      kind: "mutation",
      access: "authenticated",
      statement: "INSERT INTO photos (user_id, file_id, title, created_at) VALUES (?1, ?2, ?3, ?4)",
      bindings: [
        { source: "actor", field: "id" },
        { source: "input", key: "fileId", type: "string", maxLength: 64 },
        { source: "input", key: "title", type: "string", maxLength: 120 },
        { source: "kernel", field: "now" },
      ],
    },
    "photos.remove": {
      kind: "mutation",
      access: "authenticated",
      guard: "Only the person who added this image can remove it.",
      statement: "DELETE FROM photos WHERE file_id = ?2 AND user_id = ?1",
      bindings: [
        { source: "actor", field: "id" },
        { source: "input", key: "fileId", type: "string", maxLength: 64 },
      ],
    },
  },
};

const html = `
<title>Pictures</title>
<main class="ui-page" data-purpose="A public gallery. Signed-in people add images; every visitor sees them.">
  <header class="ui-page-header">
    <div>
      <p class="ui-eyebrow">Gallery</p>
      <h1 class="ui-title">Pictures</h1>
    </div>
    <div class="ui-identity"><span id="identity"></span><button id="sign-in" class="ui-button ui-button-sm" hidden>Sign in to add</button></div>
  </header>

  <form id="upload-form" class="ui-card ui-form" hidden>
    <label class="ui-field">Title<input class="ui-input" id="title" maxlength="120" required></label>
    <label class="ui-field">Image<input class="ui-input" id="file" type="file" accept="image/png,image/jpeg,image/gif,image/webp,image/avif" required></label>
    <button class="ui-button ui-button-primary" type="submit">Add to the gallery</button>
  </form>
  <p id="status" class="ui-status" role="status" aria-live="polite"></p>

  <div class="ui-section-head"><span>Images</span><span id="count"></span></div>
  <div id="photos" class="ui-tiles"></div>
</main>`;

const script = `${templateHelpers}
let mine = new Set();

async function load() {
  const photos = await miniapp.actions.call('photos.list');
  $('#count').textContent = photos.length + (photos.length === 1 ? ' image' : ' images');
  const urls = await Promise.all(photos.map(photo => miniapp.files.url(photo.file_id)));
  $('#photos').innerHTML = photos.length ? photos.map((photo, index) =>
    '<figure class="ui-tile"><img src="' + urls[index] + '" alt="' + escapeHtml(photo.title) + '"><figcaption><span>' + escapeHtml(photo.title) + '</span>' +
    (mine.has(photo.file_id) ? '<button class="ui-button ui-button-ghost ui-button-sm" data-remove="' + escapeHtml(photo.file_id) + '">Remove</button>' : '') +
    '</figcaption></figure>'
  ).join('') : '<p class="ui-list-empty">No images yet.</p>';
}

async function start() {
  const session = await miniapp.auth.session();
  const signedIn = session.role !== 'anonymous';
  $('#identity').textContent = signedIn ? session.name : '';
  $('#sign-in').hidden = signedIn;
  $('#upload-form').hidden = !signedIn;
  mine = new Set(signedIn ? (await miniapp.actions.call('photos.mine')).map(photo => photo.file_id) : []);
  await load();
}

$('#sign-in').addEventListener('click', async () => { await miniapp.auth.signIn(); await start(); });

$('#upload-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const file = $('#file').files[0];
  const title = $('#title').value.trim();
  if (!file || !title) return;
  try {
    say('');
    const stored = await miniapp.files.upload(file, { visibility: 'public' });
    await miniapp.actions.call('photos.add', { fileId: stored.id, title });
    event.target.reset();
    mine.add(stored.id);
    await load();
  } catch (error) { showError(error); }
});

$('#photos').addEventListener('click', async (event) => {
  const button = event.target.closest('button[data-remove]');
  if (!button) return;
  try {
    say('');
    await miniapp.actions.call('photos.remove', { fileId: button.dataset.remove });
    await miniapp.files.delete(button.dataset.remove);
    mine.delete(button.dataset.remove);
    await load();
  } catch (error) { showError(error); }
});

try { await start(); } catch (error) { showError(error); }
`;

export const galleryTemplate: TemplateSource = {
  id: "gallery",
  name: "File gallery",
  sentence: "Signed-in people add images and every visitor sees them.",
  visibility: "public",
  model,
  files: {
    "index.html": html,
    "styles.css": templateStyles,
    "app.js": script,
    "schema.sql": schema,
    "actions.json": `${JSON.stringify(actions, null, 2)}\n`,
  },
};
