// One module for both pages. Each part runs only when its elements exist.
const $ = (selector) => document.querySelector(selector);
const el = (tag, props = {}, children = []) => {
  const node = Object.assign(document.createElement(tag), props);
  node.append(...children);
  return node;
};
// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
function reason(error) {
  if (error?.message === "rate-limited") return "Too many requests just now. Wait a few seconds and try again.";
  if (error instanceof TypeError && /fetch|network|load failed/i.test(error.message)) return "The site could not reach its server. Check the connection and try again.";
  return error?.message ?? String(error);
}
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
  let photos;
  try {
    photos = await miniapp.actions.call("photos.published");
  } catch (error) {
    status.textContent = `The photographs could not be loaded. ${reason(error)}`;
    status.hidden = false;
    return;
  }
  status.textContent = photos.length ? "" : "No photographs are published yet.";
  status.hidden = photos.length > 0;
  const urls = await Promise.all(photos.map((photo) => miniapp.files.url(photo.file_id)));
  grid.replaceChildren(...photos.map((photo, index) => {
    const img = el("img", { src: urls[index], alt: photo.title, loading: "lazy", width: 640, height: 480 });
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
      status.textContent = `The message was not sent. ${reason(error)}`;
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
    const button = form.querySelector("button[type=submit]");
    button.disabled = true;
    status.textContent = "Uploading…";
    try {
      // A draft is a private file and a published photograph a public one.
      const file = await miniapp.files.upload(data.get("file"), { visibility: publish ? "public" : "private" });
      const caption = String(data.get("caption")).trim();
      try {
        // photos.add declares a guard, so an account outside the studio is refused with its sentence.
        await miniapp.actions.call("photos.add", {
          file_id: file.id, title: String(data.get("title")).trim(), caption: caption || null, status: publish ? "published" : "draft",
        });
      } catch (error) {
        // The refused photograph's file is deleted, so it does not stay behind counting against the quota.
        await miniapp.files.delete(file.id).catch(() => {});
        throw error;
      }
      form.reset();
      status.textContent = publish ? "Published." : "Saved as a draft.";
      await refreshStudio();
      if (publish) await showGallery();
    } catch (error) {
      status.textContent = `Not saved. ${reason(error)}`;
    } finally {
      button.disabled = false;
    }
  });
  await refreshStudio();
}

async function refreshStudio() {
  const [photos, messages] = await Promise.all([
    miniapp.actions.call("studio.photos"),
    miniapp.actions.call("studio.messages"),
  ]);
  const drafts = photos.filter((photo) => photo.status === "draft");
  // files.url names the file's path, so a preview loads only the drafts shown.
  const urls = await Promise.all(drafts.map((photo) => miniapp.files.url(photo.file_id)));
  $("#drafts").replaceChildren(...(drafts.length ? drafts.map((photo, index) => draftRow(photo, urls[index])) : [el("li", { className: "muted", textContent: "No drafts." })]));
  $("#inbox").replaceChildren(...(messages.length ? messages.map((message) => el("li", { className: "card" }, [
    el("p", {}, [el("strong", { textContent: message.name }), ` · ${message.email} · ${message.topic} · ${day(message.received_at)}`]),
    el("p", { textContent: message.body }),
  ])) : [el("li", { className: "muted", textContent: "No messages yet." })]));
}

function draftRow(photo, url) {
  // Publishing makes the draft's own file public, so no second copy is stored.
  const publish = el("button", { type: "button", className: "secondary", textContent: "Publish" });
  const note = el("span", { className: "muted" });
  publish.addEventListener("click", async () => {
    publish.disabled = true;
    note.textContent = "Publishing…";
    try {
      await miniapp.files.update(photo.file_id, { visibility: "public" });
      try {
        await miniapp.actions.call("photos.publish", { id: photo.id });
      } catch (error) {
        // The row stayed a draft, so its file goes back to private.
        await miniapp.files.update(photo.file_id, { visibility: "private" }).catch(() => {});
        throw error;
      }
      await refreshStudio();
      await showGallery();
    } catch (error) {
      note.textContent = `Not published. ${reason(error)}`;
      publish.disabled = false;
    }
  });
  return el("li", { className: "card draft" }, [
    url ? el("img", { src: url, alt: photo.title, width: 120, height: 90 }) : el("span", { className: "muted", textContent: "No preview" }),
    el("div", {}, [el("strong", { textContent: photo.title }), el("p", { className: "muted", textContent: photo.caption ?? "" })]),
    el("div", { className: "actions" }, [publish, note]),
  ]);
}
