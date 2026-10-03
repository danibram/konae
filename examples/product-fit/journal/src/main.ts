import { renderEntry, type Entry } from './entry';
declare const miniapp: any;
const $ = <T extends HTMLElement>(s: string) => document.querySelector(s) as T;
const moods = ['calm', 'bright', 'heavy', 'restless'];
// Every status line reads a failure the same way. A refusal's message is the
// sentence to show; the rate limit answers with a code and an unreachable
// Kernel with a browser error, so those two get a sentence here.
const reason = (error: any): string => error?.message === 'rate-limited' ? 'Too many requests just now. Wait a few seconds and try again.'
  : error instanceof TypeError && /fetch|network|load failed/i.test(error.message) ? 'The journal could not reach its server. Check the connection and try again.'
  : error?.message ?? String(error);
$('#moods').innerHTML = moods.map((m, i) => `<label><input type="radio" name="mood" value="${m}" ${i === 0 ? 'checked' : ''}><span>${m}</span></label>`).join('');
const body = $<HTMLTextAreaElement>('#body');
body.addEventListener('input', () => { $('#chars').textContent = `${body.value.length} / 2000`; });

async function load() {
  const entries: Entry[] = await miniapp.actions.call('entries.list');
  $('#entries').innerHTML = entries.map(renderEntry).join('') || '<li class="muted">Nothing yet. Write the first one.</li>';
}
$('#compose').addEventListener('submit', async (e) => {
  e.preventDefault();
  const mood = (document.querySelector('input[name=mood]:checked') as HTMLInputElement).value;
  const button = (e.target as HTMLFormElement).querySelector('button') as HTMLButtonElement;
  button.disabled = true;
  $('#status').textContent = '';
  try { await miniapp.actions.call('entries.create', { body: body.value, mood }); body.value = ''; $('#chars').textContent = '0 / 2000'; await load(); }
  catch (err) { $('#status').textContent = reason(err); }
  finally { button.disabled = false; }
});
$('#entries').addEventListener('click', async (e) => {
  const b = (e.target as HTMLElement).closest('button[data-id]') as HTMLButtonElement | null;
  if (!b) return;
  b.disabled = true;
  $('#status').textContent = '';
  // entries.delete declares a guard, so an entry deleted on another device is refused with its sentence.
  try { await miniapp.actions.call('entries.delete', { id: Number(b.dataset.id) }); }
  catch (err) { $('#status').textContent = reason(err); }
  await load().catch((err) => { $('#status').textContent = reason(err); });
});
const session = await miniapp.auth.session();
if (session.role === 'anonymous') { $('#gate').hidden = false; $('#signin').onclick = () => miniapp.auth.signIn(); }
else {
  $('#who').textContent = session.name;
  const out = document.createElement('button'); out.id = 'out'; out.textContent = 'Sign out'; $('#who').append(out);
  $('#out').onclick = async () => { await miniapp.auth.signOut(); location.reload(); };
  $('#app').hidden = false; await load().catch((err) => { $('#status').textContent = reason(err); });
}
