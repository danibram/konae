const esc = (value) => String(value ?? '').replace(/[&<>"]/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]));
const rows = await miniapp.actions.call('incidents.recent');
const open = rows.filter(r => !r.resolved_at);
const state = document.querySelector('#state'); state.textContent = open.length ? `${open.length} open incident${open.length > 1 ? 's' : ''}` : 'all systems normal'; state.className = open.length ? 'bad' : 'ok';
document.querySelector('#list').innerHTML = rows.map(r => `<li class="${r.resolved_at ? '' : 'open'}"><time datetime="${new Date(r.started_at).toISOString()}">${new Date(r.started_at).toLocaleString()}${r.resolved_at ? ' → ' + new Date(r.resolved_at).toLocaleString() : ' (ongoing)'}</time><strong>${esc(r.title)}</strong><div>${esc(r.body)}</div></li>`).join('') || '<li>No incidents recorded.</li>';
// The badge is an uploaded file. The page embeds it from the Kernel's file
// read path with its own session or, on this public app, as a visitor.
const [badge] = await miniapp.actions.call('badge.current');
if (badge) { const img = document.createElement('img'); img.src = `/_konae/files/${encodeURIComponent(badge.file_id)}`; img.alt = 'Status badge'; img.width = 32; img.height = 32; document.querySelector('#badge').append(img); }
