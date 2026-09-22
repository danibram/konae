// One module for both pages. Each part runs only when its elements exist.
const $ = (selector) => document.querySelector(selector);
const el = (tag, props = {}, children = []) => {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
};
const fileUrl = (id) => `/_littlebox/files/${encodeURIComponent(id)}`;
const day = (ms) => new Date(ms).toLocaleDateString("en-GB", { year: "numeric", month: "long", day: "numeric" });

const session = await miniapp.auth.session();
const signedIn = session.role !== "anonymous";

const authButton = $("#auth");
authButton.textContent = signedIn ? "Sign out" : "Studio sign-in";
authButton.addEventListener("click", async () => {
  if (!signedIn) return miniapp.auth.signIn();
  await miniapp.auth.signOut();
  location.reload();
});

if ($("#grid")) await showGallery();
if ($("#contact")) wireContact($("#contact"));
if ($("#studio") && signedIn) await showStudio();

async function showGallery() {
  const status = $("#gallery-status");
  const grid = $("#grid");
  const viewer = $("#viewer");
  const photos = await miniapp.actions.call("photos.published");
  status.textContent = photos.length ? "" : "No photographs are published yet.";
  status.hidden = photos.length > 0;
  grid.replaceChildren(...photos.map((photo) => {
    const img = el("img", { src: fileUrl(photo.file_id), alt: photo.title, loading: "lazy", width: 640, height: 480 });
    const open = el("button", { type: "button", className: "tile" }, [img]);
    open.addEventListener("click", () => {
      viewer.querySelector("img").src = img.src;
      viewer.querySelector("img").alt = photo.title;
      viewer.querySelector("figcaption").replaceChildren(el("strong", { textContent: photo.title }), photo.caption ? ` ${photo.caption}` : "");
      viewer.showModal();
    });
    return el("li", {}, [
      open,
      el("p", { className: "title" }, [el("strong", { textContent: photo.title }), el("span", { className: "muted", textContent: day(photo.published_at) })]),
    ]);
  }));
}

function wireContact(form) {
  const status = $("#contact-status");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const data = new FormData(form);
    const button = form.querySelector("button[type=submit]");
    button.disabled = true;
    status.className = "muted";
    status.textContent = "Sending…";
    try {
      await miniapp.actions.call("contact.send", {
        name: String(data.get("name")).trim(),
        email: String(data.get("email")).trim(),
        topic: String(data.get("topic")),
        message: String(data.get("message")).trim(),
      });
      form.reset();
      status.className = "ok";
      status.textContent = "Thank you. The studio has your message.";
    } catch (error) {
      status.className = "error";
      status.textContent = error.message === "rate-limited"
        ? "Too many messages from your network just now. Wait a few seconds and send again."
        : `The message was not sent: ${error.message}`;
    } finally {
      button.disabled = false;
    }
  });
}

async function showStudio() {
  $("#studio").hidden = false;
  const form = $("#upload");
  const status = $("#upload-status");
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const data = new FormData(form);
    const publish = data.get("publish") === "on";
    status.textContent = "Uploading…";
    try {
      // Visibility is fixed at upload: a draft is a private file, a published photograph a public one.
      const file = await miniapp.files.upload(data.get("file"), publish ? { visibility: "public" } : undefined);
      const caption = String(data.get("caption")).trim();
      const added = await miniapp.actions.call("photos.add", {
        file_id: file.id, title: String(data.get("title")).trim(), caption: caption || null, status: publish ? "published" : "draft",
      });
      if (added.changes === 0) throw Error("this account is not the studio's curator");
      form.reset();
      status.textContent = publish ? "Published." : "Saved as a draft.";
      await refreshStudio();
      if (publish) await showGallery();
    } catch (error) {
      status.textContent = `Not saved: ${error.message}`;
    }
  });
  await refreshStudio();
}

async function refreshStudio() {
  const [photos, messages, files] = await Promise.all([
    miniapp.actions.call("studio.photos"),
    miniapp.actions.call("studio.messages"),
    miniapp.files.list(),
  ]);
  const urls = new Map(files.map((file) => [file.id, file.url]));
  const drafts = photos.filter((photo) => photo.status === "draft");
  $("#drafts").replaceChildren(...(drafts.length ? drafts.map((photo) => draftRow(photo, urls.get(photo.file_id))) : [el("li", { className: "muted", textContent: "No drafts." })]));
  $("#inbox").replaceChildren(...(messages.length ? messages.map((message) => el("li", { className: "card" }, [
    el("p", {}, [el("strong", { textContent: message.name }), ` · ${message.email} · ${message.topic} · ${day(message.received_at)}`]),
    el("p", { textContent: message.body }),
  ])) : [el("li", { className: "muted", textContent: "No messages yet." })]));
}

function draftRow(photo, url) {
  // A private file cannot become public, and the page cannot read a Blob URL's
  // bytes back under connect-src 'self', so publishing asks for the file again.
  const pick = el("input", { type: "file", accept: "image/*", hidden: true });
  const publish = el("button", { type: "button", className: "secondary", textContent: "Publish…" });
  const note = el("span", { className: "muted" });
  publish.addEventListener("click", () => pick.click());
  pick.addEventListener("change", async () => {
    if (!pick.files?.[0]) return;
    note.textContent = "Publishing…";
    try {
      const file = await miniapp.files.upload(pick.files[0], { visibility: "public" });
      await miniapp.actions.call("photos.publish", { id: photo.id, file_id: file.id });
      await refreshStudio();
      await showGallery();
    } catch (error) {
      note.textContent = `Not published: ${error.message}`;
    }
  });
  return el("li", { className: "card draft" }, [
    url ? el("img", { src: url, alt: photo.title, width: 120, height: 90 }) : el("span", { className: "muted", textContent: "No preview" }),
    el("div", {}, [el("strong", { textContent: photo.title }), el("p", { className: "muted", textContent: photo.caption ?? "" })]),
    el("div", { className: "actions" }, [publish, pick, note]),
  ]);
}
