import { templateHelpers, templateStyles } from "./shared";
import type { TemplateSource } from "./types";

const schema = `-- Written by any visitor through the form. Only the accounts in admins read it.
CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  body TEXT NOT NULL,
  received_at INTEGER NOT NULL
);

-- The accounts that read what the app keeps for its staff. The owner adds one
-- from Data > SQL with the account ID that Auth > Users shows:
--   INSERT INTO admins (user_id) VALUES ('<account id>');
CREATE TABLE IF NOT EXISTS admins (
  user_id TEXT PRIMARY KEY
);
`;

const actions = {
  version: 1,
  actions: {
    "messages.send": {
      kind: "mutation",
      access: "anonymous",
      statement: "INSERT INTO messages (name, email, body, received_at) VALUES (?1, ?2, ?3, ?4)",
      bindings: [
        { source: "input", key: "name", type: "string", maxLength: 120 },
        { source: "input", key: "email", type: "string", maxLength: 254 },
        { source: "input", key: "message", type: "string", maxLength: 4000 },
        { source: "kernel", field: "now" },
      ],
      notify: {
        to: "owner",
        subject: "New message from {input.name}",
        body: "{input.name} ({input.email}) wrote:\n{input.message}\nSign in to the app to read every message.",
      },
    },
    "messages.list": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT id, name, email, body, received_at FROM messages WHERE EXISTS (SELECT 1 FROM admins WHERE user_id = ?1) ORDER BY id DESC LIMIT 100",
      bindings: [{ source: "actor", field: "id" }],
    },
    "admins.me": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT count(*) AS admin FROM admins WHERE user_id = ?1",
      bindings: [{ source: "actor", field: "id" }],
    },
  },
};

const html = `
<title>Write to us</title>
<main class="ui-page" data-purpose="A public form. Anyone can send a message; the owner gets an email and reads them here.">
  <header class="ui-page-header">
    <div>
      <p class="ui-eyebrow">Get in touch</p>
      <h1 class="ui-title">Write to us</h1>
    </div>
    <div class="ui-identity"><span id="identity"></span><button id="sign-in" class="ui-button ui-button-sm">Staff sign in</button></div>
  </header>

  <form id="message-form" class="ui-card ui-form">
    <label class="ui-field">Your name<input class="ui-input" id="name" maxlength="120" autocomplete="name" required></label>
    <label class="ui-field">Your email<input class="ui-input" id="email" type="email" maxlength="254" autocomplete="email" required></label>
    <label class="ui-field">Message<textarea class="ui-textarea" id="message" maxlength="4000" required></textarea></label>
    <button class="ui-button ui-button-primary" type="submit">Send</button>
  </form>
  <p id="status" class="ui-status" role="status" aria-live="polite"></p>

  <section id="inbox" hidden>
    <div class="ui-section-head"><span>Messages</span><span>Only the accounts in admins see these</span></div>
    <div id="messages" class="ui-list"></div>
  </section>
</main>`;

const script = `${templateHelpers}
async function start() {
  const session = await miniapp.auth.session();
  $('#identity').textContent = session.role === 'anonymous' ? '' : session.name;
  $('#sign-in').hidden = session.role !== 'anonymous';
  const admin = session.role !== 'anonymous' && (await miniapp.actions.call('admins.me'))[0].admin > 0;
  $('#inbox').hidden = !admin;
  if (!admin) return;
  const messages = await miniapp.actions.call('messages.list');
  $('#messages').innerHTML = messages.length ? messages.map(message =>
    '<article class="ui-list-item"><div class="ui-list-main"><p>' + escapeHtml(message.body) + '</p><small>' + escapeHtml(message.name) + ' · ' + escapeHtml(message.email) + ' · ' + when(message.received_at) + '</small></div></article>'
  ).join('') : '<p class="ui-list-empty">No messages yet.</p>';
}

$('#sign-in').addEventListener('click', async () => { await miniapp.auth.signIn(); await start(); });

$('#message-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  try {
    say('');
    await miniapp.actions.call('messages.send', { name: $('#name').value.trim(), email: $('#email').value.trim(), message: $('#message').value.trim() });
    event.target.reset();
    say('Sent. Thank you.', 'success');
    await start();
  } catch (error) { showError(error); }
});

try { await start(); } catch (error) { showError(error); }
`;

export const formTemplate: TemplateSource = {
  id: "form",
  name: "Public form",
  sentence: "Anyone can send a message; the owner gets an email and reads them all here.",
  visibility: "public",
  files: {
    "index.html": html,
    "styles.css": templateStyles,
    "app.js": script,
    "schema.sql": schema,
    "actions.json": `${JSON.stringify(actions, null, 2)}\n`,
  },
};
