const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
const reason = (error) => error?.message === 'rate-limited' ? 'Too many requests just now. Wait a few seconds and try again.'
  : error instanceof TypeError && /fetch|network|load failed/i.test(error.message) ? 'The app could not reach its server. Check the connection and try again.'
  : error?.message ?? String(error);
const session = await miniapp.auth.session();
const signedIn = session.role !== 'anonymous';
$('#who').textContent = signedIn ? `Signed in as ${session.name}.` : '';
$('#editor').hidden = !signedIn;
async function render() {
  const pieces = await miniapp.actions.call('pieces.list');
  // The uploaded photo stays private to the potter, so only her own pieces name their file;
  // a visitor gets no file ID and sees the inline copy. files.url names the file's path,
  // so the page loads only the photos it shows instead of downloading every file.
  const mine = signedIn ? await miniapp.actions.call('pieces.files') : [];
  const fileOf = new Map(mine.map(p => [p.id, p.file_id]));
  $('#gallery').innerHTML = pieces.map(p => {
    const file = fileOf.get(p.id);
    const img = file ? `<img src="/_konae/files/${encodeURIComponent(file)}" alt="${esc(p.title)}">` : p.image_data !== '-' ? `<img src="${esc(p.image_data)}" alt="${esc(p.title)}">` : `<div class="nophoto">photo only visible when signed in</div>`;
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
  const button = e.target.querySelector('button'); button.disabled = true;
  let uploaded = null;
  try {
    const file = $('#photo').files[0];
    const inline = file && $('#inline').checked ? await shrink(file) : '-';
    uploaded = file ? await miniapp.files.upload(file) : null;
    await miniapp.actions.call('pieces.create', { title: $('#title').value.trim(), body: $('#body').value.trim() || '-', fileId: uploaded?.id ?? '-', imageData: inline });
    $('#status').textContent = 'Published.'; e.target.reset(); await render();
  } catch (err) {
    // A photo whose piece was refused is deleted, so it does not count against the quota.
    if (uploaded) await miniapp.files.delete(uploaded.id).catch(() => {});
    $('#status').textContent = reason(err);
  } finally { button.disabled = false; }
});
await render().catch((err) => { $('#gallery').textContent = reason(err); });
