const list = document.querySelector("#entries");
const entries = await window.miniapp.actions.call("entries.list", {});
for (const entry of entries) {
  const item = document.createElement("li");
  item.textContent = `${entry.name}: ${entry.body}`;
  list?.append(item);
}
document.querySelector("#app")?.setAttribute("data-ready", "true");
