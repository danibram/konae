import { limitIndex, unboundedQueryReason } from "./query-bound.ts";
import { sqlTokens } from "./sql-tokens.ts";
import { words } from "./i18n.js";

const { protocol, hostname, port, pathname } = location;
const lang = document.documentElement.lang === "es" ? "es" : "en";
if (pathname === "/" && new URLSearchParams(location.search).get("lang") === "es") location.replace("/es/");
const domain = hostname;
const apps = document.documentElement.dataset.apps || domain;
const at = (host, path) => `${protocol}//${host}${port && (host !== apps || apps === domain) ? `:${port}` : ""}${path}`;
const docs = (page) => at(domain, `/docs/${page}`);
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const links = {
  editor: at(`app.${domain}`, "/"),
  mcp: at(`app.${domain}`, "/mcp"),
  "editor-origin": at(`app.${domain}`, ""),
  docs: docs(""),
  llms: at(domain, "/llms.txt"),
  "first-app": docs("guides/first-app-from-the-editor"),
  move: docs("guides/take-the-app-elsewhere"),
  checks: docs("gate-and-kernel"),
  cli: docs("guides/edit-files-with-the-cli"),
  operate: docs("operate"),
  "mcp-guide": docs("reference"),
  pieces: docs("pieces"),
  invite: at(apps, "/p/invitation/"),
  privacy: at(domain, "/politica-de-privacidad"),
  terms: at(domain, "/terminos-y-condiciones"),
  examples: docs("examples"),
};
const point = () => {
  for (const element of document.querySelectorAll("[data-link]")) {
    element.href = element.dataset.link === "invite" ? `${links.invite}?lang=${lang}` : links[element.dataset.link];
  }
};
point();
for (const element of document.querySelectorAll("[data-text]")) element.textContent = links[element.dataset.text];

for (const pre of document.querySelectorAll("figure.code pre")) {
  pre.innerHTML = pre.innerHTML.split("\n").map((line) => `<span class="line">${line.replace(/(--[a-z-]+)/g, '<span class="flag">$1</span>') || " "}</span>`).join("");
}

const w = words[lang];
const redraw = [];

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

for (const button of document.querySelectorAll("[data-copy]")) {
  const pre = button.closest("figure").querySelector("pre");
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

const doors = document.querySelector(".doors");
const light = (on) => (event) => {
  if (!event.target.closest?.("mark")) return;
  for (const mark of doors.querySelectorAll("mark")) mark.classList.toggle("linked", on);
};
doors.addEventListener("mouseover", light(true));
doors.addEventListener("mouseout", light(false));

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
  toggle.addEventListener("click", () => {
    const [code, ...comment] = input.value.trimEnd().split(/(?=--)/);
    const statement = code.trimEnd();
    const bare = statement.replace(/\s+LIMIT\s+\S+$/i, "");
    input.value = [bare === statement ? `${statement} LIMIT 14` : bare, ...comment].join(" ");
    judge();
  });
  redraw.push(judge);
}

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

const bar = document.querySelector(".bar");
const stick = () => bar.classList.toggle("stuck", scrollY > 8);
addEventListener("scroll", stick, { passive: true });
stick();
const heroCta = document.querySelector(".hero-actions");
const barCta = document.querySelector(".bar-cta");
if (heroCta && barCta && "IntersectionObserver" in window) {
  new IntersectionObserver((entries) => barCta.classList.toggle("quiet", entries.some((entry) => entry.isIntersecting))).observe(heroCta);
}


for (const draw of redraw) draw();
