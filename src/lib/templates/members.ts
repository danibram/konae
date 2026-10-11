import { templateHelpers, templateStyles } from "./shared";
import type { TemplateSource } from "./types";

/**
 * Every staff action checks the caller's role with this one predicate. The
 * owner's role comes from the Kernel through me.join; admin comes from the
 * owner's promote. The Kernel accepts it as written, with ?1 bound to actor.id.
 */
export const adminCheck = "(SELECT role FROM profiles WHERE user_id = ?1) IN ('admin', 'owner')";

const schema = `-- One row per person who joined. The Kernel says who is signed in and which
-- account is the owner's; the role column is the app's own decision.
-- Staff actions check: ${adminCheck}
CREATE TABLE IF NOT EXISTS profiles (
  user_id TEXT PRIMARY KEY NOT NULL,
  display_name TEXT NOT NULL CHECK (length(trim(display_name)) BETWEEN 1 AND 40),
  role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member', 'admin', 'owner')),
  joined_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS posts (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  author_id TEXT NOT NULL,
  body TEXT NOT NULL CHECK (length(trim(body)) BETWEEN 1 AND 2000),
  created_at INTEGER NOT NULL
);
`;

const model = `import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const profiles = sqliteTable("profiles", {
  userId: text("user_id").primaryKey(),
  displayName: text("display_name").notNull(),
  role: text("role").notNull().default("member"),
  joinedAt: integer("joined_at").notNull(),
});

export const posts = sqliteTable("posts", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  authorId: text("author_id").notNull(),
  body: text("body").notNull(),
  createdAt: integer("created_at").notNull(),
});
`;

const actor = { source: "actor", field: "id" };
const role = { source: "actor", field: "role" };
const now = { source: "kernel", field: "now" };
const userId = { source: "input", key: "userId", type: "string", maxLength: 64 };

const actions = {
  version: 1,
  actions: {
    "me.join": {
      kind: "mutation",
      access: "authenticated",
      statement: "INSERT INTO profiles (user_id, display_name, role, joined_at) VALUES (?1, ?2, CASE WHEN ?3 = 'owner' THEN 'owner' ELSE 'member' END, ?4) ON CONFLICT (user_id) DO NOTHING",
      bindings: [actor, { source: "input", key: "displayName", type: "string", maxLength: 40 }, role, now],
    },
    "me.get": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT user_id, display_name, role FROM profiles WHERE user_id = ?1 LIMIT 1",
      bindings: [actor],
    },
    "posts.list": {
      kind: "query",
      access: "authenticated",
      statement: "SELECT p.id, p.body, p.created_at, p.author_id, a.display_name AS author FROM posts p LEFT JOIN profiles a ON a.user_id = p.author_id ORDER BY p.id DESC LIMIT 50",
      bindings: [],
    },
    "posts.add": {
      kind: "mutation",
      access: "authenticated",
      statement: "INSERT INTO posts (author_id, body, created_at) VALUES (?1, ?2, ?3)",
      bindings: [actor, { source: "input", key: "body", type: "string", maxLength: 2000 }, now],
    },
    "posts.remove": {
      kind: "mutation",
      access: "authenticated",
      guard: "Only its author or an admin can remove a post.",
      statement: `DELETE FROM posts WHERE id = ?2 AND (author_id = ?1 OR ${adminCheck})`,
      bindings: [actor, { source: "input", key: "id", type: "number" }],
    },
    "members.list": {
      kind: "query",
      access: "authenticated",
      statement: `SELECT user_id, display_name, role, joined_at FROM profiles WHERE ${adminCheck} ORDER BY display_name LIMIT 100`,
      bindings: [actor],
    },
    "members.promote": {
      kind: "mutation",
      access: "authenticated",
      guard: "Only the owner can make a member an admin.",
      statement: "UPDATE profiles SET role = 'admin' WHERE user_id = ?1 AND role = 'member' AND ?2 = 'owner'",
      bindings: [userId, role],
    },
    "members.demote": {
      kind: "mutation",
      access: "authenticated",
      guard: "Only the owner can return an admin to member.",
      statement: "UPDATE profiles SET role = 'member' WHERE user_id = ?1 AND role = 'admin' AND ?2 = 'owner'",
      bindings: [userId, role],
    },
  },
};

const html = `
<main class="app-shell" data-purpose="A members area. Members post, admins moderate, the owner names the admins.">
  <header>
    <div>
      <p class="eyebrow">Members</p>
      <h1>The clubhouse</h1>
    </div>
    <div class="identity"><span id="identity"></span><button id="sign-in" class="quiet" hidden>Sign in</button><button id="sign-out" class="quiet" hidden>Sign out</button></div>
  </header>

  <p id="landing" class="intro" hidden>Sign in to read and write posts. The owner decides who is an admin.</p>
  <p id="status" class="status" role="status" aria-live="polite"></p>

  <section id="member-area" hidden>
    <form id="post-form" class="card">
      <label>New post<textarea id="post-body" maxlength="2000" required></textarea></label>
      <button class="primary" type="submit">Post</button>
    </form>
    <div class="section-label"><span>Posts</span><span id="role"></span></div>
    <div id="posts" class="rows"></div>
  </section>

  <section id="staff-area" hidden>
    <div class="section-label"><span>Members</span><span>Only the owner changes roles</span></div>
    <div id="members" class="rows"></div>
  </section>
</main>`;

const script = `${templateHelpers}
let me = null;

async function loadPosts() {
  const posts = await miniapp.actions.call('posts.list');
  const staff = me.role !== 'member';
  $('#posts').innerHTML = posts.length ? posts.map(post =>
    '<article class="row"><div><p>' + escapeHtml(post.body) + '</p><small>' + escapeHtml(post.author ?? 'Someone') + ' · ' + when(post.created_at) + '</small></div>' +
    (staff || post.author_id === me.user_id ? '<button data-remove="' + post.id + '">Remove</button>' : '<span></span>') +
    '</article>'
  ).join('') : '<p class="empty">No posts yet.</p>';
}

async function loadMembers() {
  const members = await miniapp.actions.call('members.list');
  const owner = me.role === 'owner';
  $('#members').innerHTML = members.map(member =>
    '<article class="row"><div><p>' + escapeHtml(member.display_name) + '</p><small>' + member.role + '</small></div>' +
    (owner && member.role === 'member' ? '<button data-promote="' + escapeHtml(member.user_id) + '">Make admin</button>'
      : owner && member.role === 'admin' ? '<button data-demote="' + escapeHtml(member.user_id) + '">Make member</button>' : '<span></span>') +
    '</article>'
  ).join('');
}

async function start() {
  const session = await miniapp.auth.session();
  const signedIn = session.role !== 'anonymous';
  $('#identity').textContent = signedIn ? session.name : '';
  $('#sign-in').hidden = signedIn;
  $('#sign-out').hidden = !signedIn;
  $('#landing').hidden = signedIn;
  $('#member-area').hidden = !signedIn;
  if (!signedIn) return;
  await miniapp.actions.call('me.join', { displayName: session.name.split('@')[0].slice(0, 40) || 'Member' });
  [me] = await miniapp.actions.call('me.get');
  $('#role').textContent = 'You are ' + me.role;
  $('#staff-area').hidden = me.role === 'member';
  await loadPosts();
  if (me.role !== 'member') await loadMembers();
}

$('#sign-in').addEventListener('click', async () => { await miniapp.auth.signIn(); await start(); });
$('#sign-out').addEventListener('click', async () => { await miniapp.auth.signOut(); await start(); });

$('#post-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const body = $('#post-body').value.trim();
  if (!body) return;
  try {
    statusLine.textContent = '';
    await miniapp.actions.call('posts.add', { body });
    $('#post-body').value = '';
    await loadPosts();
  } catch (error) { showError(error); }
});

document.addEventListener('click', async (event) => {
  const button = event.target.closest('button[data-remove], button[data-promote], button[data-demote]');
  if (!button) return;
  try {
    statusLine.textContent = '';
    if (button.dataset.remove) await miniapp.actions.call('posts.remove', { id: Number(button.dataset.remove) });
    if (button.dataset.promote) await miniapp.actions.call('members.promote', { userId: button.dataset.promote });
    if (button.dataset.demote) await miniapp.actions.call('members.demote', { userId: button.dataset.demote });
    await start();
  } catch (error) { showError(error); }
});

try { await start(); } catch (error) { showError(error); }
`;

export const membersTemplate: TemplateSource = {
  id: "members",
  name: "Members with roles",
  sentence: "People sign in and post; the owner makes some of them admins who can moderate.",
  model,
  files: {
    "index.html": html,
    "styles.css": templateStyles,
    "app.js": script,
    "schema.sql": schema,
    "actions.json": `${JSON.stringify(actions, null, 2)}\n`,
  },
};
