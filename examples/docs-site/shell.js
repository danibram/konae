// Every page is the same shell. It fetches the page's markdown, which the
// build publishes under content/, and renders it here. No inline scripts:
// a published app runs under script-src 'self'.
import { marked } from "marked";
import { slug } from "./slug.js";

const $ = (selector) => document.querySelector(selector);
const esc = (value) => String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const { root, route } = document.body.dataset;
const content = $("#content");
const sidebar = $("#sidebar");
const tocBox = $("#toc");
/** The address of a page of this site, relative to the page that is open. */
const pageHref = (to) => (to === "index" ? root || "./" : root + to);

sidebar.querySelector('[aria-current="page"]')?.scrollIntoView({ block: "nearest" });

/**
 * Points a link written for the repository at this site. A link to another
 * page's .md file becomes that page's address. A link to a file the site does
 * not hold goes to the repository when site.json names one, and otherwise
 * loses its href and stays as text.
 */
function rewriteLink(link, site) {
  const href = link.getAttribute("href") ?? "";
  if (!href || href.startsWith("#") || /^[a-z][a-z0-9+.-]*:/i.test(href)) return;
  const target = new URL(href, `http://repo/docs/${site.pages[route].source}`);
  const file = decodeURIComponent(target.pathname.slice(1));
  const to = file.startsWith("docs/") ? site.routes[file.slice(5)] : undefined;
  if (to) link.setAttribute("href", pageHref(to) + target.hash);
  else if (site.repository) link.setAttribute("href", `${site.repository}/blob/main/${file}${target.hash}`);
  else link.replaceWith(...link.childNodes);
}

function decorate(site) {
  const seen = new Map();
  for (const h of content.querySelectorAll("h1, h2, h3, h4, h5, h6")) h.id = slug(h.textContent, seen);
  const lead = content.querySelector("h1 + p");
  lead?.classList.add("lead");
  for (const quote of content.querySelectorAll("blockquote")) quote.classList.add("note");
  for (const code of content.querySelectorAll(":not(pre) > code")) if (code.textContent.length <= 28) code.classList.add("short");
  for (const table of content.querySelectorAll("table")) {
    const wrap = document.createElement("div");
    wrap.className = "table-wrap";
    table.replaceWith(wrap);
    wrap.append(table);
    // A narrow screen shows each row as a card, so every cell carries its column's name.
    const names = [...table.querySelectorAll("thead th")].map((th) => th.textContent.trim());
    for (const row of table.querySelectorAll("tbody tr")) [...row.cells].forEach((cell, index) => { if (names[index]) cell.dataset.label = names[index]; });
  }
  for (const h of content.querySelectorAll("h2")) if (h.textContent.trim() === "Read next") h.classList.add("readnext");
  for (const link of content.querySelectorAll("a")) rewriteLink(link, site);
  for (const pre of content.querySelectorAll("pre")) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "copy";
    button.textContent = "Copy";
    button.addEventListener("click", async () => {
      const text = pre.querySelector("code")?.innerText ?? "";
      try { await navigator.clipboard.writeText(text.replace(/\n$/, "")); button.textContent = "Copied"; }
      catch {
        const range = document.createRange();
        range.selectNodeContents(pre.querySelector("code") ?? pre);
        getSelection().removeAllRanges();
        getSelection().addRange(range);
        button.textContent = /Mac|iPhone|iPad/.test(navigator.platform) ? "Press ⌘C" : "Press Ctrl+C";
      }
      setTimeout(() => (button.textContent = "Copy"), 1400);
    });
    pre.append(button);
  }
}

function tableOfContents() {
  const headings = [...content.querySelectorAll("h2[id]")];
  if (headings.length < 2) return;
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

/** One script tag for the diagram renderer, which the build publishes as it is under vendor/. */
let renderer;
const loadRenderer = () => (renderer ??= new Promise((resolve, reject) => {
  const script = document.createElement("script");
  script.src = `${root}vendor/mermaid.js`;
  script.onload = () => resolve(globalThis.mermaid);
  script.onerror = () => reject(Error("The diagram renderer did not load."));
  document.head.append(script);
}));

/** The diagram colours are the design system's tokens as the page resolves them now, in light or dark. */
function diagramTheme() {
  const style = getComputedStyle(document.documentElement);
  const token = (name, fallback) => style.getPropertyValue(name).trim() || fallback;
  const text = token("--ds-text", "#24251f");
  const surface = token("--ds-surface", "#f1efe7");
  const line = token("--ds-muted", "#62665a");
  return {
    darkMode: matchMedia("(prefers-color-scheme: dark)").matches,
    background: token("--ds-canvas", "#dedbd0"),
    fontFamily: style.getPropertyValue("--ds-font").trim() || "system-ui, sans-serif",
    fontSize: "14px",
    primaryColor: surface, primaryTextColor: text, primaryBorderColor: text,
    secondaryColor: token("--ds-subtle", "#e8e5db"), secondaryTextColor: text, secondaryBorderColor: line,
    tertiaryColor: token("--ds-canvas", "#dedbd0"), tertiaryTextColor: text, tertiaryBorderColor: line,
    lineColor: line, textColor: text, mainBkg: surface, nodeBorder: text,
    clusterBkg: token("--ds-subtle", "#e8e5db"), clusterBorder: line, titleColor: text,
    edgeLabelBackground: token("--ds-canvas", "#dedbd0"),
    actorBkg: surface, actorBorder: text, actorTextColor: text, actorLineColor: line,
    signalColor: text, signalTextColor: text, labelBoxBkgColor: surface, labelBoxBorderColor: line, labelTextColor: text,
    loopTextColor: text, noteBkgColor: token("--ds-accent-soft", "#e4f6a8"), noteTextColor: text, noteBorderColor: line,
    activationBkgColor: token("--ds-accent-soft", "#e4f6a8"), activationBorderColor: text, sequenceNumberColor: surface,
  };
}

/**
 * Draws each mermaid block of the page in place. A page without one loads no
 * renderer. A block that does not parse stays as its source with the reason.
 */
async function diagrams() {
  const blocks = [...content.querySelectorAll("pre > code.language-mermaid")];
  if (!blocks.length) return;
  const figures = blocks.map((code) => {
    const figure = document.createElement("figure");
    figure.className = "diagram";
    figure.dataset.source = code.textContent;
    figure.setAttribute("role", "img");
    // The paragraph under a diagram says what it shows, so it names the figure for a screen reader.
    const caption = code.parentElement.nextElementSibling;
    if (caption?.tagName === "P") { caption.id ||= `diagram-${blocks.indexOf(code) + 1}-text`; figure.setAttribute("aria-labelledby", caption.id); }
    code.parentElement.replaceWith(figure);
    return figure;
  });
  let mermaid;
  // The renderer measures each label in the page's typeface, so the typeface loads first.
  try { [mermaid] = await Promise.all([loadRenderer(), document.fonts?.ready]); }
  catch (error) { for (const figure of figures) showSource(figure, error.message); return; }
  let drawn = 0;
  const draw = async () => {
    mermaid.initialize({
      startOnLoad: false, securityLevel: "strict", theme: "base", themeVariables: diagramTheme(),
      flowchart: { htmlLabels: true, curve: "basis", padding: 14, nodeSpacing: 28, rankSpacing: 44, wrappingWidth: 280, useMaxWidth: true },
      sequence: { useMaxWidth: true, mirrorActors: false, messageMargin: 28, boxMargin: 8, actorMargin: 30, wrap: true, width: 200 },
    });
    for (const figure of figures) {
      try {
        const { svg } = await mermaid.render(`diagram-${++drawn}`, figure.dataset.source);
        figure.innerHTML = svg;
        const enlarge = document.createElement("button");
        enlarge.type = "button";
        enlarge.className = "enlarge";
        enlarge.textContent = "Enlarge";
        enlarge.addEventListener("click", () => zoom(figure));
        figure.append(enlarge);
      } catch (error) {
        showSource(figure, `This diagram did not draw: ${error?.message ?? error}`);
      }
    }
  };
  await draw();
  matchMedia("(prefers-color-scheme: dark)").addEventListener("change", draw);
}

/**
 * Opens a diagram over the page at the size it was drawn for, so a narrow
 * column or a phone can read it. The drawing itself moves into the dialog and
 * back, because its styles hang on its id.
 */
function zoom(figure) {
  const drawing = figure.querySelector("svg");
  const dialog = document.createElement("dialog");
  dialog.className = "diagram-zoom";
  const shown = { maxWidth: drawing.style.maxWidth, width: drawing.style.width };
  figure.style.minHeight = `${figure.offsetHeight}px`;
  drawing.style.maxWidth = "none";
  drawing.style.width = `${Math.round(drawing.viewBox.baseVal.width * 1.25)}px`;
  const close = document.createElement("button");
  close.type = "button";
  close.className = "enlarge";
  close.textContent = "Close";
  close.addEventListener("click", () => dialog.close());
  dialog.append(close, drawing);
  dialog.addEventListener("click", (event) => { if (event.target === dialog) dialog.close(); });
  dialog.addEventListener("close", () => {
    Object.assign(drawing.style, shown);
    figure.prepend(drawing);
    figure.style.minHeight = "";
    dialog.remove();
  });
  document.body.append(dialog);
  dialog.showModal();
}

function showSource(figure, reason) {
  const pre = document.createElement("pre");
  pre.textContent = figure.dataset.source;
  const note = document.createElement("figcaption");
  note.textContent = reason;
  figure.replaceChildren(pre, note);
}

async function render() {
  const [site, page] = await Promise.all([fetch(`${root}content/site.json`), fetch(`${root}content/${route}.md`)]);
  if (!site.ok || !page.ok) throw Error(`This page's markdown did not load (HTTP ${page.ok ? site.status : page.status}).`);
  content.innerHTML = marked.parse(await page.text(), { gfm: true });
  decorate(await site.json());
  tableOfContents();
  diagrams();
  content.removeAttribute("aria-busy");
  // The browser looked for the anchor before the headings existed.
  if (location.hash.length > 1) document.getElementById(decodeURIComponent(location.hash.slice(1)))?.scrollIntoView();
}
if (route) render().catch((error) => {
  content.querySelector(".loading")?.replaceChildren(error.message);
  content.removeAttribute("aria-busy");
});

// The installation's home answers on the bare domain. The docs answer on
// <domain>/p/<name>/ or on their own <slug>-<6 hex>.<domain>, so the domain
// comes from location, as examples/home/home.js reads it.
{
  const labels = location.hostname.split(".");
  const own = !location.pathname.startsWith("/p/") && labels.length > 1 && /-[0-9a-f]{6}$/.test(labels[0]);
  const domain = own ? labels.slice(1).join(".") : location.hostname;
  for (const link of document.querySelectorAll("[data-installation-home]")) {
    link.href = `${location.protocol}//${domain}${location.port ? `:${location.port}` : ""}/`;
    link.hidden = false;
  }
}

// Narrow screens.
const menu = $(".menu");
const scrim = $("#scrim");
const closeNav = () => { document.body.classList.remove("nav-open"); scrim.hidden = true; menu.setAttribute("aria-expanded", "false"); menu.textContent = "Menu"; };
const openNav = () => {
  document.body.classList.add("nav-open"); scrim.hidden = false; menu.setAttribute("aria-expanded", "true"); menu.textContent = "Close";
  sidebar.scrollTop = 0;
};
menu.addEventListener("click", () => (document.body.classList.contains("nav-open") ? closeNav() : openNav()));
scrim.addEventListener("click", closeNav);
sidebar.addEventListener("click", (event) => { if (event.target.closest("a")) closeNav(); });

// Marks each match of the query in text that is escaped first.
function mark(text, query) {
  const safe = esc(text);
  const needle = esc(query.trim());
  if (!needle) return safe;
  return safe.replace(new RegExp(`[\\w-]*${needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}[\\w-]*`, "gi"), (match) => `<mark>${match}</mark>`);
}

// A snippet starts and ends on a whole word, with an ellipsis where the section goes on.
function snippet(r) {
  let text = String(r.snippet ?? "").replace(/\s+/g, " ").trim();
  if (r.clipped) text = `…${text.slice(text.indexOf(" ") + 1)}`;
  if (r.cut && text.includes(" ")) text = `${text.slice(0, text.lastIndexOf(" "))}…`;
  return text;
}

// Search runs the anonymous docs.search action over one row per section.
const search = $("#search");
const results = $("#results");
let timer;
let selected = -1;
async function run() {
  const q = search.value.trim();
  if (!q) { results.hidden = true; results.innerHTML = ""; return; }
  let html;
  try {
    const rows = await miniapp.actions.call("docs.search", { q });
    if (search.value.trim() !== q) return;
    // A section title that appears twice in the results carries its page's name.
    const counts = new Map();
    for (const r of rows) counts.set(r.title, (counts.get(r.title) ?? 0) + 1);
    const count = `<div class="count">${rows.length === 12 ? "The first 12 results" : `${rows.length} ${rows.length === 1 ? "result" : "results"}`} for “${esc(q)}”</div>`;
    html = rows.length
      ? count + rows.map((r, i) => {
          const [to, anchor] = r.path.split("#");
          return `<a href="${esc(pageHref(to) + (anchor ? `#${anchor}` : ""))}" role="option" aria-selected="${i === 0}">${r.page === r.title ? "" : `<span class="where">${esc(r.page)}</span>`}<span class="what">${mark(counts.get(r.title) > 1 && r.page !== r.title ? `${r.title}, ${r.page}` : r.title, q)}</span><span class="snip">${mark(snippet(r), q)}</span></a>`;
        }).join("")
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
