// Shared by every page. No inline scripts: published apps run under script-src 'self'.
const $ = (selector) => document.querySelector(selector);
const esc = (value) => String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const content = $("#content");
const sidebar = $("#sidebar");
const tocBox = $("#toc");

// Give every h2 an id when the author left it out, mark "Read next", add copy buttons.
for (const h of content.querySelectorAll("h2:not([id])")) {
  h.id = h.textContent.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}
for (const h of content.querySelectorAll("h2")) if (h.textContent.trim() === "Read next") h.classList.add("readnext");
for (const pre of content.querySelectorAll("pre")) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "copy";
  button.textContent = "Copy";
  button.addEventListener("click", async () => {
    const text = pre.querySelector("code")?.innerText ?? pre.innerText.replace(/Copy$/, "");
    try { await navigator.clipboard.writeText(text.replace(/\n$/, "")); button.textContent = "Copied"; }
    catch { button.textContent = "Select and copy"; }
    setTimeout(() => (button.textContent = "Copy"), 1400);
  });
  pre.append(button);
}

// Current page in the sidebar.
const clean = (path) => path.replace(/\.html$/, "").replace(/\/index$/, "/").replace(/(.)\/$/, "$1");
const here = clean(location.pathname);
for (const link of sidebar.querySelectorAll("a")) {
  if (clean(new URL(link.href).pathname) === here) link.setAttribute("aria-current", "page");
  else link.removeAttribute("aria-current");
}
sidebar.querySelector('[aria-current="page"]')?.scrollIntoView({ block: "nearest" });

// On-page table of contents from h2[id].
const headings = [...content.querySelectorAll("h2[id]")];
if (headings.length >= 2) {
  tocBox.innerHTML = `<h2>On this page</h2><ul>${headings.map((h) => `<li><a href="#${esc(h.id)}" data-id="${esc(h.id)}">${esc(h.textContent)}</a></li>`).join("")}</ul>`;
  const links = new Map([...tocBox.querySelectorAll("a")].map((a) => [a.dataset.id, a]));
  const spy = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      for (const link of links.values()) link.classList.remove("active");
      links.get(entry.target.id)?.classList.add("active");
    }
  }, { rootMargin: "-70px 0px -70% 0px" });
  headings.forEach((h) => spy.observe(h));
}

// Narrow screens.
const menu = $(".menu");
const scrim = $("#scrim");
const closeNav = () => { document.body.classList.remove("nav-open"); scrim.hidden = true; menu.setAttribute("aria-expanded", "false"); };
const openNav = () => { document.body.classList.add("nav-open"); scrim.hidden = false; menu.setAttribute("aria-expanded", "true"); };
menu.addEventListener("click", () => (document.body.classList.contains("nav-open") ? closeNav() : openNav()));
scrim.addEventListener("click", closeNav);
sidebar.addEventListener("click", (event) => { if (event.target.closest("a")) closeNav(); });

// Search runs the anonymous pages.search action.
const search = $("#search");
const results = $("#results");
let timer;
let selected = -1;
async function run() {
  const q = search.value.trim();
  if (!q) { results.hidden = true; results.innerHTML = ""; return; }
  let html;
  try {
    const rows = await miniapp.actions.call("pages.search", { q });
    if (search.value.trim() !== q) return;
    html = rows.length
      ? rows.map((r, i) => `<a href="${esc(r.path)}" role="option" aria-selected="${i === 0}"><span class="where">${esc(r.path)}</span><span class="what">${esc(r.title)}</span><span class="snip">${esc(r.snippet)}</span></a>`).join("")
      : '<div class="none">No page matches.</div>';
    selected = rows.length ? 0 : -1;
  } catch (error) {
    html = `<div class="none">${esc(error?.message ?? "Search is unavailable.")}</div>`;
    selected = -1;
  }
  results.innerHTML = html;
  results.hidden = false;
}
function move(delta) {
  const items = [...results.querySelectorAll("a")];
  if (!items.length) return;
  selected = (selected + delta + items.length) % items.length;
  items.forEach((item, i) => item.setAttribute("aria-selected", String(i === selected)));
  items[selected].scrollIntoView({ block: "nearest" });
}
search.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(run, 150); });
search.addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown") { event.preventDefault(); move(1); }
  else if (event.key === "ArrowUp") { event.preventDefault(); move(-1); }
  else if (event.key === "Enter") { results.querySelector('a[aria-selected="true"]')?.click(); }
  else if (event.key === "Escape") { results.hidden = true; search.blur(); }
});
document.addEventListener("click", (event) => { if (!event.target.closest(".search")) results.hidden = true; });
document.addEventListener("keydown", (event) => {
  if (event.key === "/" && !/^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName ?? "")) { event.preventDefault(); search.focus(); }
});
