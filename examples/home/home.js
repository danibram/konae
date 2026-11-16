import { limitIndex, unboundedQueryReason } from "./query-bound.ts";
import { sqlTokens } from "./sql-tokens.ts";
import { es, words } from "./i18n.js";

// Points the links at this installation. The page answers on <domain>/,
// on <domain>/p/<name>/ and on its own <slug>-<6 hex>.<domain>, and only the
// browser knows which, so the addresses come from location. A custom domain
// looks like the bare domain here, so on one the links name that domain.
const { protocol, hostname, port, pathname } = location;
const labels = hostname.split(".");
const ownHostname = !pathname.startsWith("/p/") && labels.length > 1 && /-[0-9a-f]{6}$/.test(labels[0]);
const domain = ownHostname ? labels.slice(1).join(".") : hostname;
const at = (host, path) => `${protocol}//${host}${port ? `:${port}` : ""}${path}`;
const docs = (page) => at(domain, `/p/docs/${page}`);
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const links = {
  editor: at(`app.${domain}`, "/"),
  mcp: at(`app.${domain}`, "/mcp"),
  "editor-origin": at(`app.${domain}`, ""),
  docs: docs(""),
  llms: docs("llms.txt"),
  "first-app": docs("guides/first-app-from-the-editor"),
  move: docs("guides/take-the-app-elsewhere"),
  checks: docs("gate-and-kernel"),
  cli: docs("guides/edit-files-with-the-cli"),
  operate: docs("operate"),
  "mcp-guide": docs("reference"),
  pieces: docs("pieces"),
  invite: at(domain, "/p/invitation/"),
  privacy: at(domain, "/p/politica-de-privacidad/"),
  terms: at(domain, "/p/terminos-y-condiciones/"),
  examples: docs("examples"),
};
const point = () => {
  for (const element of document.querySelectorAll("[data-link]")) element.href = links[element.dataset.link];
};
point();
for (const element of document.querySelectorAll("[data-text]")) element.textContent = links[element.dataset.text];

// Each line of a code block becomes its own element before the page's text is
// read, so a long command wraps under itself and never reads as a new one, and
// a flag such as --url never breaks between its hyphens.
for (const pre of document.querySelectorAll("figure.code pre")) {
  pre.innerHTML = pre.innerHTML.split("\n").map((line) => `<span class="line">${line.replace(/(--[a-z-]+)/g, '<span class="flag">$1</span>') || " "}</span>`).join("");
}

// The page is written in English and carries its Spanish text in i18n.js.
// ?lang= wins, then the visitor's last choice, then the browser's language.
// Each part of the page that writes text registers a function in `redraw`,
// which runs again when the language changes.
const marked = [...document.querySelectorAll("[data-t]")];
const english = new Map(marked.map((element) => [element, element.innerHTML]));
const stored = (() => { try { return localStorage.getItem("konae-home-lang"); } catch { return null; } })();
const asked = new URLSearchParams(location.search).get("lang");
let lang = [asked, stored, navigator.language?.slice(0, 2)].find((value) => value === "en" || value === "es") ?? "en";
let w = words[lang];
const redraw = [];
const speak = () => {
  w = words[lang];
  document.documentElement.lang = lang;
  document.title = w.title;
  document.querySelector('meta[name="description"]').content = w.description;
  for (const element of marked) element.innerHTML = lang === "es" && es[element.dataset.t] ? es[element.dataset.t] : english.get(element);
  for (const element of document.querySelectorAll("[data-t-label]")) element.setAttribute("aria-label", w.labels[element.dataset.tLabel] ?? element.getAttribute("aria-label"));
  point();
  // The invitation request opens in the language the visitor reads here.
  for (const element of document.querySelectorAll('[data-link="invite"]')) element.href = `${links.invite}?lang=${lang}`;
  for (const draw of redraw) draw();
};
const switcher = document.querySelector("[data-lang]");
redraw.push(() => { switcher.textContent = w.other; switcher.lang = lang === "es" ? "en" : "es"; });
switcher.hidden = false;
switcher.addEventListener("click", () => {
  lang = lang === "es" ? "en" : "es";
  try { localStorage.setItem("konae-home-lang", lang); } catch {}
  const here = new URL(location.href);
  here.searchParams.set("lang", lang);
  history.replaceState(null, "", here);
  speak();
});

// The example app. The editor's preview and the app that changes server show
// the same list, so a name added in one is still there in the other.
const here = domain === "localhost" || /^[\d.]+$/.test(domain) ? "konae.example.com" : domain;
const hosts = () => [here, w.otherHost];
const places = 14;
const list = { dana: false, names: [], moved: false };
const total = () => 9 + (list.dana ? 1 : 0) + list.names.length;
const short = (name) => (name.length > 14 ? `${name.slice(0, 14).replace(/\s+\S*$/, "")}…` : name);
const views = [...document.querySelectorAll("[data-appview]")].map((root) => {
  root.innerHTML = '<p class="app-title"></p><p class="app-meta"></p><div class="app-meter" aria-hidden="true"><span></span></div><ul class="app-rows"></ul>';
  return { root, kind: root.dataset.appview, title: root.children[0], meta: root.children[1], meter: root.querySelector(".app-meter span"), rows: root.children[3] };
});
const row = (text, note, cls) => {
  const li = document.createElement("li");
  li.textContent = text;
  if (cls) li.className = cls;
  if (note) { const span = document.createElement("span"); span.textContent = note; li.append(span); }
  return li;
};
const renderList = () => {
  const latest = list.names[list.names.length - 1];
  for (const view of views) {
    const note = view.kind === "stage" && list.moved ? w.still : w.joined;
    view.title.textContent = w.appTitle;
    view.meta.textContent = w.places(total(), places);
    view.meter.style.width = `${(total() / places) * 100}%`;
    const shown = 2 + (list.dana ? 1 : 0) + (latest ? 1 : 0);
    view.rows.replaceChildren(
      row("Ana"), row("Bruno"),
      ...(list.dana ? [row("Dana", note, "new")] : []),
      ...(latest ? [row(short(latest), note, "new")] : []),
      row(w.more(total() - shown), "", "rest"),
    );
  }
};
redraw.push(renderList);

// The editor in the hero builds the app once when it comes into view: the
// sentence types itself, the assistant answers, the files appear, the
// Validation Gate's line shows, Apply puts the app in the preview and Deploy
// gives it its address. Each moment is a step number on the figure, and the
// stylesheet shows what belongs to that step.
const studio = document.querySelector("[data-studio]");
if (studio) {
  const typed = studio.querySelector("[data-typed]");
  const ph = studio.querySelector("[data-ph]");
  const you = studio.querySelector("[data-you]");
  const apply = studio.querySelector("[data-apply]");
  const deploy = studio.querySelector("[data-deploy]");
  const send = studio.querySelector(".send");
  const badge = studio.querySelector("[data-badge]");
  const url = studio.querySelector("[data-live-url]");
  const replay = studio.querySelector("[data-replay]");
  const form = studio.querySelector("[data-join]");
  const field = form.querySelector("input");
  const add = form.querySelector("[data-add]");
  const formNote = studio.querySelector("[data-join-note]");
  const announce = studio.querySelector("[data-announce]");
  let live = false;
  let running = false;
  let formSays = "";
  const step = (n) => { studio.dataset.step = String(n); };
  const press = async (element) => { element.classList.add("pressed"); await wait(160); element.classList.remove("pressed"); };
  const draw = () => {
    const full = total() >= places;
    badge.textContent = live ? w.published : w.preview;
    badge.classList.toggle("live", live);
    deploy.textContent = live ? "Deploy changes" : "Deploy";
    deploy.classList.toggle("done", live);
    url.textContent = `${w.slug}.${here}`;
    replay.textContent = w.playAgain;
    field.placeholder = w.yourName;
    field.disabled = full;
    add.textContent = full ? w.startOver : w.signMeUp;
    formNote.textContent = full ? w.full : formSays ? w[formSays] : "";
  };
  redraw.push(draw);
  const finish = () => { live = true; list.dana = true; step(7); renderList(); draw(); replay.hidden = false; };
  const play = async () => {
    if (running) return;
    running = true;
    replay.hidden = true;
    live = false;
    list.dana = false;
    step(0);
    renderList();
    draw();
    const sentence = you.textContent;
    ph.hidden = true;
    studio.classList.add("typing");
    for (let i = 1; i <= sentence.length; i += 1) { typed.textContent = sentence.slice(0, i); await wait(18); }
    await wait(350);
    await press(send);
    studio.classList.remove("typing");
    typed.textContent = "";
    ph.hidden = false;
    step(1);
    await wait(450);
    step(2);
    await wait(1300);
    step(3);
    await wait(800);
    step(4);
    await wait(1000);
    step(5);
    await wait(1100);
    await press(apply);
    step(6);
    await wait(1300);
    await press(deploy);
    live = true;
    step(7);
    draw();
    url.classList.add("fresh");
    setTimeout(() => url.classList.remove("fresh"), 1400);
    await wait(1300);
    list.dana = true;
    renderList();
    announce.textContent = w.saidJoined("Dana", `${w.slug}.${here}`);
    running = false;
    replay.hidden = false;
  };
  replay.addEventListener("click", play);
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    formSays = "";
    if (total() >= places) { list.names = []; field.value = ""; renderList(); draw(); field.focus(); return; }
    const name = field.value.trim().slice(0, 16);
    if (!name) { formSays = "typeName"; draw(); field.focus(); return; }
    field.value = "";
    list.names.push(name);
    renderList();
    draw();
    announce.textContent = w.saidJoined(name, `${w.slug}.${here}`);
  });
  if (reduced || !("IntersectionObserver" in window)) finish();
  else {
    step(0);
    const seen = new IntersectionObserver((entries) => {
      if (!entries.some((entry) => entry.isIntersecting)) return;
      seen.disconnect();
      setTimeout(play, 500);
    }, { threshold: 0.25 });
    seen.observe(studio);
  }
}

// The app changes server when the visitor presses the button. The slot that
// holds the app is as tall as the app and the empty one folds to one line;
// both change together while a copy of the card flies between them, so the
// figure keeps its height.
const stage = document.querySelector("[data-stage]");
if (stage) {
  const app = stage.querySelector("[data-app]");
  const slots = [...stage.querySelectorAll("[data-slot] .slot")];
  const url = stage.querySelector("[data-url]");
  const names0 = stage.querySelectorAll("[data-host]");
  const chip = stage.querySelector("[data-package]");
  const mover = stage.querySelector("[data-move]");
  const visited = new Set([0]);
  let where = 0;
  let busy = false;
  let arrived = false;
  const reserve = () => {
    const pad = parseFloat(getComputedStyle(slots[0]).paddingTop) + parseFloat(getComputedStyle(slots[0]).paddingBottom);
    stage.style.setProperty("--slot-full", `${app.offsetHeight + pad}px`);
  };
  const draw = () => {
    for (const [i, name] of [...names0].entries()) name.textContent = hosts()[i];
    url.textContent = `${w.slug}.${hosts()[where]}`;
    chip.textContent = arrived ? w.arrived : w.package;
    for (const [i, slot] of slots.entries()) slot.querySelector(".slot-empty").textContent = visited.has(i) ? w.emptyNow : w.emptyYet;
    mover.textContent = busy ? w.moving : where === 1 ? w.moveBack : w.moveTo(hosts()[1]);
    if (busy) mover.setAttribute("aria-disabled", "true"); else mover.removeAttribute("aria-disabled");
    if (!busy) reserve();
  };
  const place = (index) => {
    where = index;
    slots[index].prepend(app);
    url.textContent = `${w.slug}.${hosts()[index]}`;
    for (const [i, slot] of slots.entries()) slot.classList.toggle("empty", i !== index);
    visited.add(index);
  };
  const carry = async (index) => {
    if (reduced) { place(index); return; }
    const from = app.getBoundingClientRect();
    const base = stage.getBoundingClientRect();
    // Measure where the app lands without animating, then start from the old layout.
    stage.classList.add("instant");
    place(index);
    const to = app.getBoundingClientRect();
    place(1 - index);
    void stage.offsetHeight;
    stage.classList.remove("instant");
    const ghost = app.cloneNode(true);
    ghost.removeAttribute("data-app");
    ghost.classList.add("ghost");
    ghost.setAttribute("aria-hidden", "true");
    Object.assign(ghost.style, { left: `${from.left - base.left}px`, top: `${from.top - base.top}px`, width: `${from.width}px` });
    stage.append(ghost);
    app.style.visibility = "hidden";
    // Out of the flow while it flies, so each slot follows only its own animated height.
    app.style.position = "absolute";
    app.style.width = `${from.width}px`;
    place(index);
    chip.classList.add(index === 1 ? "down" : "up");
    await wait(20);
    ghost.style.scale = "0.94";
    await wait(180);
    ghost.style.translate = `0px ${to.top - from.top}px`;
    await wait(210);
    ghost.querySelector("[data-url]").textContent = `${w.slug}.${hosts()[index]}`;
    await wait(210);
    ghost.style.scale = "1";
    await wait(260);
    app.style.position = "";
    app.style.width = "";
    app.style.visibility = "";
    ghost.remove();
    chip.classList.remove("down", "up");
    url.classList.add("fresh");
    setTimeout(() => url.classList.remove("fresh"), 1200);
  };
  mover.addEventListener("click", async () => {
    if (busy) return;
    busy = true;
    arrived = false;
    stage.classList.remove("arrived");
    draw();
    await carry(1 - where);
    arrived = true;
    list.moved = true;
    stage.classList.add("arrived");
    busy = false;
    renderList();
    draw();
  });
  place(0);
  mover.hidden = false;
  redraw.push(draw);
  addEventListener("resize", () => { if (!busy) reserve(); });
}

// Every code block copies its own text. Without clipboard access it selects the text instead.
for (const button of document.querySelectorAll("[data-copy]")) {
  const pre = button.closest("figure").querySelector("pre");
  // The copied text is read when the button is pressed, one line per element,
  // so it follows the language on screen.
  const text = () => [...pre.querySelectorAll(".line")].map((line) => line.textContent).join("\n");
  const label = () => {
    const state = button.dataset.state;
    button.textContent = state === "copied" ? w.copied : state === "press" ? w.press(/Mac|iPhone|iPad/.test(navigator.platform) ? "⌘C" : "Ctrl+C") : w.copy;
  };
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text());
      button.dataset.state = "copied";
    } catch {
      const range = document.createRange();
      range.selectNodeContents(pre);
      getSelection().removeAllRanges();
      getSelection().addRange(range);
      button.dataset.state = "press";
    }
    label();
    setTimeout(() => { button.dataset.state = ""; label(); }, 2400);
  });
  redraw.push(label);
}

// The three ways for developers sit behind tabs. Without this script they
// stay one under the other.
const tabs = document.querySelector("[data-tabs]");
if (tabs) {
  const buttons = [...tabs.querySelectorAll('[role="tab"]')];
  const panels = buttons.map((button) => document.getElementById(button.getAttribute("aria-controls")));
  const select = (index, focus) => {
    for (const [i, button] of buttons.entries()) {
      button.setAttribute("aria-selected", String(i === index));
      button.tabIndex = i === index ? 0 : -1;
      panels[i].hidden = i !== index;
    }
    if (focus) buttons[index].focus();
  };
  for (const [i, button] of buttons.entries()) {
    button.addEventListener("click", () => select(i));
    button.addEventListener("keydown", (event) => {
      if (event.key === "ArrowRight") select((i + 1) % buttons.length, true);
      if (event.key === "ArrowLeft") select((i + buttons.length - 1) % buttons.length, true);
    });
  }
  select(0);
}

// Pointing at "anyone with the link" lights the action's "access": "anonymous"
// too, and back. A change of language writes the first mark again, so the
// marks are found each time.
const doors = document.querySelector(".doors");
const light = (on) => (event) => {
  if (!event.target.closest?.("mark")) return;
  for (const mark of doors.querySelectorAll("mark")) mark.classList.toggle("linked", on);
};
doors.addEventListener("mouseover", light(true));
doors.addEventListener("mouseout", light(false));

// The statement box runs the row-limit check the editor's Validation Gate
// runs (query-bound.ts is a copy of src/lib/query-bound.ts), with the sentence
// src/kernel/actions.ts and the Kernel build from it. The Gate checks a
// statement's shape first; this box says so instead of judging other statements.
const check = document.querySelector("[data-check]");
if (check) {
  const toggle = check.querySelector("[data-toggle]");
  const input = check.querySelector("textarea");
  const verdict = check.querySelector("[data-verdict]");
  const who = check.querySelector("[data-who]");
  const answer = check.querySelector("[data-answer]");
  const foot = check.querySelector(".statement-foot span");
  const accepted = (sql) => {
    const top = sqlTokens(sql).filter((token) => token.depth === 0);
    const at = limitIndex(top);
    if (at !== -1) {
      const count = top[at + 1]?.text;
      if (!/^\d+$/.test(count ?? "")) return w.namesLimit;
      return w.readsAtMost(count);
    }
    if (!top.some((token) => token.kind === "word" && token.text.toUpperCase() === "FROM")) return w.noTable;
    return w.oneRow;
  };
  const judge = () => {
    const sql = input.value.trim();
    const select = /^(select|with)\b/i.test(sql);
    const reason = select ? unboundedQueryReason(sql) : null;
    check.classList.toggle("dropped", Boolean(reason));
    check.classList.toggle("idle", !select);
    verdict.textContent = !select ? w.notChecked : reason ? w.refused : w.accepted;
    who.textContent = reason ? w.byBothExact : !select ? w.rowCheck : w.byBoth;
    input.placeholder = w.placeholder;
    answer.textContent = !sql
      ? w.writeSelect
      : !select
      ? w.onlySelect
      : reason
        ? `Action players.list ${reason} Add a LIMIT. A total (count, sum, avg, min or max) with no GROUP BY does not need one.`
        : accepted(sql);
    const limited = select && !reason && limitIndex(sqlTokens(sql).filter((token) => token.depth === 0)) !== -1;
    toggle.textContent = reason ? w.addLimit : w.removeLimit;
    foot.textContent = w.editOr;
    foot.parentElement.hidden = !(reason || limited);
  };
  check.querySelector("[data-static]").hidden = true;
  input.hidden = false;
  toggle.hidden = false;
  input.addEventListener("input", judge);
  // The LIMIT goes before a trailing comment, where the Gate would read it, and comes out the same way.
  toggle.addEventListener("click", () => {
    const [code, ...comment] = input.value.trimEnd().split(/(?=--)/);
    const statement = code.trimEnd();
    const bare = statement.replace(/\s+LIMIT\s+\S+$/i, "");
    input.value = [bare === statement ? `${statement} LIMIT 14` : bare, ...comment].join(" ");
    judge();
  });
  redraw.push(judge);
}

// Blocks rise into place the first time they come into view, each a little
// after the one before it in the same row. Without the observer, or with
// reduced motion, nothing is hidden.
const rising = [...document.querySelectorAll("[data-reveal]")];
if (!reduced && "IntersectionObserver" in window) {
  document.documentElement.classList.add("anim");
  const rise = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      entry.target.classList.add("seen");
      rise.unobserve(entry.target);
    }
  }, { rootMargin: "0px 0px -8% 0px" });
  for (const element of rising) {
    const siblings = [...element.parentElement.children].filter((child) => child.hasAttribute("data-reveal"));
    element.style.setProperty("--i", String(siblings.indexOf(element) % 4));
    rise.observe(element);
  }
}

// The header draws its rule once the page has scrolled, and its button hides
// while the hero's own call to action is on screen.
const bar = document.querySelector(".bar");
const stick = () => bar.classList.toggle("stuck", scrollY > 8);
addEventListener("scroll", stick, { passive: true });
stick();
const heroCta = document.querySelector(".hero-actions");
const barCta = document.querySelector(".bar-cta");
if (heroCta && barCta && "IntersectionObserver" in window) {
  new IntersectionObserver((entries) => barCta.classList.toggle("quiet", entries.some((entry) => entry.isIntersecting))).observe(heroCta);
}

// The privacy and terms links show only where the installation has those
// pages. The check needs the same origin, so it is skipped on the app's own hostname.
if (!ownHostname) for (const element of document.querySelectorAll("[data-legal]")) {
  fetch(links[element.dataset.link], { method: "HEAD" }).then((response) => { element.hidden = !response.ok; }).catch(() => {});
}

speak();
