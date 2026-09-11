const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const session = await miniapp.auth.session();
const signedIn = session.role !== 'anonymous';
$('#who').textContent = signedIn ? `Signed in as ${session.name}.` : '';
$('#editor').hidden = !signedIn;
async function render() {
  const pieces = await miniapp.actions.call('pieces.list');
  // Private uploads stay with the person who sent them. Visitors still see image_data.
  // A file uploaded as public is in this list for everyone.
  const files = await miniapp.files.list().catch(() => []);
  $('#gallery').innerHTML = pieces.map(p => {
    const file = files.find(f => f.id === p.file_id);
    const img = file ? `<img src="${file.url}" alt="${esc(p.title)}">` : p.image_data !== '-' ? `<img src="${esc(p.image_data)}" alt="${esc(p.title)}">` : `<div class="nophoto">photo only visible when signed in</div>`;
    return `<article class="piece">${img}<h3>${esc(p.title)}</h3><p>${esc(p.body)}</p></article>`;
  }).join('') || '<p class="muted">No pieces yet.</p>';
}
async function shrink(file) {
  const bitmap = await createImageBitmap(file); const c = document.createElement('canvas'); const s = 320 / Math.max(bitmap.width, bitmap.height);
  c.width = Math.round(bitmap.width * s); c.height = Math.round(bitmap.height * s); c.getContext('2d').drawImage(bitmap, 0, 0, c.width, c.height);
  return c.toDataURL('image/jpeg', 0.7);
}
$('#post')?.addEventListener('submit', async (e) => {
  e.preventDefault(); $('#status').textContent = 'Uploading…';
  try {
    const file = $('#photo').files[0];
    const uploaded = file ? await miniapp.files.upload(file) : null;
    const inline = file && $('#inline').checked ? await shrink(file) : '-';
    await miniapp.actions.call('pieces.create', { title: $('#title').value, body: $('#body').value || '-', fileId: uploaded?.id ?? '-', imageData: inline });
    $('#status').textContent = uploaded ? `Published. Upload id ${uploaded.id}, ${uploaded.size} bytes.` : 'Published.'; e.target.reset(); await render();
  } catch (err) { $('#status').textContent = err.message; }
});
await render();
