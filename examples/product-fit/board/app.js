const $ = (s) => document.querySelector(s);
const esc = (s) => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
const reason = (error) => error?.message === 'rate-limited' ? 'Too many requests just now. Wait a few seconds and try again.'
  : error instanceof TypeError && /fetch|network|load failed/i.test(error.message) ? 'The app could not reach its server. Check the connection and try again.'
  : error?.message ?? String(error);
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
  $('#members').innerHTML = members.map(m => `<li>${esc(m.display)}${m.role === 'admin' ? '<b>admin</b>' : (me.role === 'admin' ? ` <button data-promote="${m.id}">make admin</button>` : '')}</li>`).join('');
}
// Each mutation declares a guard, so a call the SQL refuses rejects with the app's own sentence.
const act = async (name, input) => { try { $('#status').textContent = ''; await miniapp.actions.call(name, input); await render(); } catch (e) { $('#status').textContent = reason(e); } };
$('#add').addEventListener('submit', async (e) => {
  e.preventDefault();
  const button = e.target.querySelector('button'); button.disabled = true;
  try { await act('cards.create', { title: $('#title').value.trim() }); $('#title').value = ''; } finally { button.disabled = false; }
});
document.addEventListener('click', (e) => {
  const b = e.target.closest('button'); if (!b) return;
  if (b.dataset.move) act('cards.move', { id: Number(b.dataset.id), status: b.dataset.move });
  if (b.dataset.del) act('cards.delete', { id: Number(b.dataset.del) });
  if (b.dataset.promote) act('members.promote', { id: Number(b.dataset.promote) });
});
await render();
