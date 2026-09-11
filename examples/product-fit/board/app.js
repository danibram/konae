const $ = (s) => document.querySelector(s);
const esc = (s) => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const session = await miniapp.auth.session();
// The private app shell loads after login.
// The server still authorizes by actor.id when the client supplies a display name.
await miniapp.actions.call('me.join', { display: session.name });
let me;
async function render() {
  [me] = await miniapp.actions.call('me.get');
  $('#who').innerHTML = `${esc(me.display)} (${me.role}) <button id="out">Sign out</button>`;
  $('#out').onclick = async () => { await miniapp.auth.signOut(); location.reload(); };
  const [cards, members] = await Promise.all([miniapp.actions.call('cards.list'), miniapp.actions.call('members.list')]);
  const statuses = ['todo', 'doing', 'done'];
  $('#columns').innerHTML = statuses.map(s => `<div class="col"><h3>${s}</h3>${cards.filter(c => c.status === s).map(c => `<div class="card">${esc(c.title)}<small>${esc(c.owner ?? '?')}</small>
    ${statuses.filter(t => t !== s).map(t => `<button data-move="${t}" data-id="${c.id}">→ ${t}</button>`).join('')}${me.role === 'admin' ? `<button class="del" data-del="${c.id}">delete</button>` : ''}</div>`).join('')}</div>`).join('');
  $('#members').innerHTML = members.map(m => `<li>${esc(m.display)}${m.role === 'admin' ? '<b>admin</b>' : (me.role === 'admin' ? ` <button data-promote="${esc(m.display)}">make admin</button>` : '')}</li>`).join('');
}
const act = async (name, input) => { try { $('#status').textContent = ''; const r = await miniapp.actions.call(name, input); if (r && r.changes === 0) $('#status').textContent = 'Not allowed: nothing changed.'; await render(); } catch (e) { $('#status').textContent = e.message; } };
$('#add').addEventListener('submit', async (e) => { e.preventDefault(); await act('cards.create', { title: $('#title').value.trim() }); $('#title').value = ''; });
document.addEventListener('click', (e) => {
  const b = e.target.closest('button'); if (!b) return;
  if (b.dataset.move) act('cards.move', { id: Number(b.dataset.id), status: b.dataset.move });
  if (b.dataset.del) act('cards.delete', { id: Number(b.dataset.del) });
  if (b.dataset.promote) act('members.promote', { display: b.dataset.promote });
});
await render();
