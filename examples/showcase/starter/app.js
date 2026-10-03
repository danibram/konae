// Starter: a public landing, a private space per member, a team area and an
// admin area. The page only asks; every rule lives in actions.json, where the
// Kernel binds the caller's ID and the SQL decides what each role may read or
// change. Hiding a section here is a courtesy, never the protection.

const UPDATES_PAGE = 10; // the LIMIT in updates.list
const NOTES_PAGE = 20; // the LIMIT in notes.list
const RANK = { anonymous: 0, member: 1, admin: 2, owner: 3 };

const $ = (selector) => document.querySelector(selector);
const call = (name, input = {}) => miniapp.actions.call(name, input);

const state = {
  session: null,
  me: null,
  updates: { before: null, q: null },
  notes: { before: null, q: null },
};

// ---------------------------------------------------------------- helpers

/** Builds an element. Text always goes through textContent, never HTML. */
function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (value !== undefined && value !== null && value !== false) node.setAttribute(key, value === true ? "" : value);
  }
  node.append(...children.filter((child) => child !== null && child !== undefined));
  return node;
}

const when = (ms) => new Date(ms).toLocaleString("en", { dateStyle: "medium", timeStyle: "short" });
const bytes = (size) => size < 1024 ? `${size} B` : size < 1048576 ? `${(size / 1024).toFixed(1)} KiB` : `${(size / 1048576).toFixed(1)} MiB`;
const nullable = (value) => value.trim() === "" ? null : value.trim();

/** Shows a result or a refusal to the person, in words the Kernel or the app's guard chose. */
function notify(message, kind = "info") {
  const notice = $("#notice");
  notice.textContent = message;
  notice.dataset.kind = kind;
  notice.hidden = false;
}

/** Runs one change, reports it, and reloads what it touched. A refusal is shown, not thrown. */
async function act(task, done, ...reloads) {
  try {
    await task();
    if (done) notify(done, "success");
  } catch (error) {
    notify(error instanceof Error ? error.message : String(error), "error");
  }
  await Promise.all(reloads.map((reload) => reload()));
}

function button(text, onclick, variant = "", label) {
  return el("button", { type: "button", class: `button ${variant}`.trim(), text, "aria-label": label, onclick });
}

async function avatar(fileId, name) {
  if (!fileId) return el("span", { class: "avatar avatar-empty", "aria-hidden": "true", text: (name || "?").slice(0, 1).toUpperCase() });
  return el("img", { class: "avatar", alt: `${name}'s avatar`, src: await miniapp.files.url(fileId) });
}

function fill(list, empty, items) {
  list.replaceChildren(...items);
  empty.hidden = items.length > 0;
}

// ------------------------------------------------------------ public landing

async function loadStatus() {
  const rows = await call("status.list");
  fill($("#status-list"), $("#status-empty"), rows.map((row) => el("li", { class: "row" },
    el("span", { class: `dot ${row.state}`, "aria-hidden": "true" }),
    el("strong", { text: row.component }),
    el("span", { class: `badge ${row.state}`, text: row.state }),
    el("span", { class: "muted grow", text: row.detail }),
    el("time", { class: "muted small", datetime: new Date(row.updated_at).toISOString(), text: when(row.updated_at) }),
  )));
}

async function loadUpdates(reset = true) {
  if (reset) state.updates.before = null;
  const rows = await call("updates.list", { before: state.updates.before, q: state.updates.q });
  const items = rows.map((row) => el("li", { class: "row stacked" },
    el("div", { class: "row-head" },
      el("strong", { text: row.title }),
      el("span", { class: "badge", text: row.source === "program" ? "from a program" : row.author ?? "team" }),
      el("time", { class: "muted small", datetime: new Date(row.published_at).toISOString(), text: when(row.published_at) }),
    ),
    row.body ? el("p", { class: "body", text: row.body }) : null,
  ));
  const list = $("#updates-list");
  if (reset) fill(list, $("#updates-empty"), items);
  else list.append(...items);
  if (rows.length) state.updates.before = rows.at(-1).id;
  $("#updates-more").hidden = rows.length < UPDATES_PAGE;
}

async function loadDownloads() {
  const rows = await call("downloads.list");
  fill($("#downloads-list"), $("#downloads-empty"), await Promise.all(rows.map(async (row) => el("li", { class: "row" },
    el("a", {
      href: await miniapp.files.url(row.file_id), download: row.name, text: row.name,
      onclick: () => { call("downloads.opened", { fileId: row.file_id }).catch(() => {}); },
    }),
    el("span", { class: "muted small", text: bytes(row.size) }),
    el("span", { class: "muted small grow", text: row.owner_name ? `shared by ${row.owner_name}` : "" }),
  ))));
}

// ----------------------------------------------------------------- my space

async function loadProfile() {
  [state.me] = await call("me.get");
  $("#display-name").value = state.me?.display_name ?? "";
}

async function loadNotes(reset = true) {
  if (reset) state.notes.before = null;
  const rows = await call("notes.list", { before: state.notes.before, q: state.notes.q });
  const items = rows.map((note) => el("li", { class: "row stacked" },
    el("div", { class: "row-head" },
      el("strong", { text: note.title }),
      el("time", { class: "muted small", datetime: new Date(note.created_at).toISOString(), text: when(note.created_at) }),
      button("Delete", () => act(() => call("notes.remove", { id: note.id }), "Note deleted.", loadNotes), "ghost", `Delete note ${note.title}`),
    ),
    note.body ? el("p", { class: "body", text: note.body }) : null,
  ));
  const list = $("#notes-list");
  if (reset) fill(list, $("#notes-empty"), items);
  else list.append(...items);
  if (rows.length) state.notes.before = rows.at(-1).id;
  $("#notes-more").hidden = rows.length < NOTES_PAGE;
}

async function loadAttachments() {
  const rows = await call("attachments.list");
  fill($("#attachments-list"), $("#attachments-empty"), await Promise.all(rows.map(async (file) => el("li", { class: "row" },
    el("a", { href: await miniapp.files.url(file.file_id), download: file.name, text: file.name }),
    el("span", { class: "muted small", text: bytes(file.size) }),
    el("span", { class: `badge ${file.public ? "operational" : ""}`.trim(), text: file.public ? "public" : "private" }),
    el("span", { class: "grow" }),
    file.public ? null : button("Publish", () => act(async () => {
      // Two steps: the Kernel makes the bytes public, then the app lists the row.
      await miniapp.files.update(file.file_id, { visibility: "public" });
      await call("attachments.publish", { id: file.id });
    }, `${file.name} is public now.`, loadAttachments, loadDownloads), "", `Publish ${file.name}`),
    button("Delete", () => act(async () => {
      // The bytes first, so the quota is freed; a file already gone still lets the row go.
      await miniapp.files.delete(file.file_id).catch((error) => { if (error.message !== "File not found.") throw error; });
      await call("attachments.remove", { id: file.id });
    }, `${file.name} deleted.`, loadAttachments, loadDownloads), "danger", `Delete ${file.name}`),
  ))));
}

// --------------------------------------------------------------------- team

async function loadTeam() {
  const [roster, posts] = await Promise.all([call("team.roster"), call("team.posts", { before: null })]);
  $("#roster").replaceChildren(...await Promise.all(roster.map(async (person) => el("li", { class: "row" },
    await avatar(person.avatar_file_id, person.display_name),
    el("strong", { text: person.display_name }),
    el("span", { class: "badge", text: person.role }),
  ))));
  fill($("#team-list"), $("#team-empty"), posts.map((post) => el("li", { class: "row stacked" },
    el("div", { class: "row-head" },
      el("strong", { text: post.author ?? "former member" }),
      el("time", { class: "muted small", datetime: new Date(post.created_at).toISOString(), text: when(post.created_at) }),
    ),
    el("p", { class: "body", text: post.body }),
  )));
}

// -------------------------------------------------------------------- admin

async function loadAdmin(q = null) {
  const [[stats], members] = await Promise.all([call("admin.stats"), call("members.list", { q })]);
  $("#stats").replaceChildren(...Object.entries(stats ?? {}).map(([name, value]) => el("div", {}, el("dt", { text: name }), el("dd", { text: String(value) }))));
  fill($("#members-list"), $("#members-empty"), await Promise.all(members.map(async (person) => el("li", { class: "row" },
    await avatar(person.avatar_file_id, person.display_name),
    el("strong", { text: person.display_name }),
    el("span", { class: "badge", text: person.role }),
    el("span", { class: "muted small grow", text: `joined ${when(person.joined_at)}` }),
    person.role === "member" ? button("Make team admin", () => act(() => call("members.promote", { userId: person.user_id }), `${person.display_name} is a team admin.`, loadAdmin, loadTeam), "", `Make ${person.display_name} a team admin`) : null,
    person.role === "admin" ? button("Return to member", () => act(() => call("members.demote", { userId: person.user_id }), `${person.display_name} is a member again.`, loadAdmin, loadTeam), "", `Return ${person.display_name} to member`) : null,
  ))));
}

// ------------------------------------------------------------------ session

async function render() {
  const signedIn = state.session.role !== "anonymous";
  const role = signedIn ? state.me?.role ?? "member" : "anonymous";
  for (const node of document.querySelectorAll("[data-needs]")) node.hidden = RANK[role] < RANK[node.dataset.needs];
  $("#sign-in").hidden = signedIn;
  $("#sign-out").hidden = !signedIn;
  $("#who-name").textContent = signedIn ? state.me?.display_name ?? state.session.name : "";
  $("#who-role").hidden = !signedIn;
  $("#who-role").textContent = role;
  const picture = $("#who-avatar");
  picture.hidden = !state.me?.avatar_file_id;
  if (state.me?.avatar_file_id) {
    picture.src = await miniapp.files.url(state.me.avatar_file_id);
    picture.alt = `${state.me.display_name}'s avatar`;
  }
  const loads = [loadStatus(), loadUpdates(), loadDownloads()];
  if (signedIn) loads.push(loadNotes(), loadAttachments());
  if (RANK[role] >= RANK.admin) loads.push(loadTeam());
  if (role === "owner") loads.push(loadAdmin());
  await Promise.all(loads);
}

async function start() {
  state.session = await miniapp.auth.session();
  state.me = null;
  if (state.session.role !== "anonymous") {
    // Joining is idempotent: the first visit creates the profile, later ones change nothing.
    await call("me.join", { displayName: state.session.name.split("@")[0].slice(0, 40) });
    await loadProfile();
  }
  await render();
}

// ------------------------------------------------------------------- events

$("#sign-in").addEventListener("click", () => miniapp.auth.signIn());
$("#sign-out").addEventListener("click", () => act(() => miniapp.auth.signOut(), "Signed out.", start));

$("#updates-search").addEventListener("submit", (event) => {
  event.preventDefault();
  state.updates.q = nullable($("#updates-q").value);
  act(() => loadUpdates());
});
$("#updates-more").addEventListener("click", () => act(() => loadUpdates(false)));

$("#notes-search").addEventListener("submit", (event) => {
  event.preventDefault();
  state.notes.q = nullable($("#notes-q").value);
  act(() => loadNotes());
});
$("#notes-more").addEventListener("click", () => act(() => loadNotes(false)));

$("#members-search").addEventListener("submit", (event) => {
  event.preventDefault();
  act(() => loadAdmin(nullable($("#members-q").value)));
});

$("#rename").addEventListener("submit", (event) => {
  event.preventDefault();
  act(() => call("me.rename", { displayName: $("#display-name").value.trim() }), "Name saved.", loadProfile, render);
});

$("#avatar-file").addEventListener("change", (event) => {
  const [file] = event.target.files;
  if (!file) return;
  const previous = state.me?.avatar_file_id;
  act(async () => {
    const stored = await miniapp.files.upload(file, { visibility: "public" });
    await call("me.avatar", { fileId: stored.id });
    // The old picture would keep counting against the app's 20 MiB.
    if (previous) await miniapp.files.delete(previous).catch(() => {});
  }, "Avatar updated.", loadProfile, render);
  event.target.value = "";
});

$("#attachment-file").addEventListener("change", (event) => {
  const [file] = event.target.files;
  if (!file) return;
  act(async () => {
    const stored = await miniapp.files.upload(file);
    await call("attachments.add", { fileId: stored.id, name: stored.name, size: stored.size });
  }, `${file.name} attached. Only you can open it.`, loadAttachments);
  event.target.value = "";
});

$("#note-add").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.target;
  act(async () => {
    await call("notes.add", { title: form.elements.title.value.trim(), body: nullable(form.elements.body.value) });
    form.reset();
  }, "Note added.", loadNotes);
});

$("#team-post").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.target;
  act(async () => {
    await call("team.post", { body: form.elements.body.value.trim() });
    form.reset();
  }, "Posted to the team.", loadTeam);
});

$("#update-publish").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = event.target;
  act(async () => {
    await call("updates.publish", { title: form.elements.title.value.trim(), body: nullable(form.elements.body.value) });
    form.reset();
  }, "Update published.", loadUpdates);
});

start().catch((error) => notify(error instanceof Error ? error.message : String(error), "error"));
