const esc = (value) => String(value ?? '').replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
// Shared by every page: wires search to the pages.search action over the runtime MCP endpoint.
const box = document.querySelector('#q'), out = document.querySelector('#hits');
let t; box?.addEventListener('input', () => { clearTimeout(t); t = setTimeout(async () => {
  const q = box.value.trim(); if (!q) { out.innerHTML = ''; return; }
  try { const rows = await miniapp.actions.call('pages.search', { q }); out.innerHTML = rows.map(r => `<li><a href="${esc(r.path)}">${esc(r.title)}</a> <small>${esc(r.snippet)}</small></li>`).join('') || '<li>No matches.</li>'; }
  catch (e) { out.innerHTML = `<li>${esc(e.message)}</li>`; }
}, 150); });
