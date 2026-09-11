const esc = (value) => String(value ?? '').replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
const rows = await miniapp.actions.call('incidents.recent');
const open = rows.filter(r => !r.resolved_at);
const state = document.querySelector('#state'); state.textContent = open.length ? `${open.length} open incident${open.length > 1 ? 's' : ''}` : 'all systems normal'; state.className = open.length ? 'bad' : 'ok';
document.querySelector('#list').innerHTML = rows.map(r => `<li class="${r.resolved_at ? '' : 'open'}"><time>${new Date(r.started_at).toISOString()}${r.resolved_at ? ' → ' + new Date(r.resolved_at).toISOString() : ' (ongoing)'}</time><strong>${esc(r.title)}</strong><div>${esc(r.body)}</div></li>`).join('') || '<li>No incidents recorded.</li>';
