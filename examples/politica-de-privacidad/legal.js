// Points the bar links at this installation. The page answers on
// <domain>/p/<name>/ and on its own hostname, so the domain comes from location.
const { protocol, hostname, port, pathname } = location;
const labels = hostname.split(".");
const ownHostname = !pathname.startsWith("/p/") && labels.length > 1 && /-[0-9a-f]{6}$/.test(labels[0]);
const domain = ownHostname ? labels.slice(1).join(".") : hostname;
const at = (host, path) => `${protocol}//${host}${port ? `:${port}` : ""}${path}`;

const links = {
  home: at(domain, "/"),
  terms: at(domain, "/p/terminos-y-condiciones/"),
};
for (const element of document.querySelectorAll("[data-link]")) {
  const href = links[element.dataset.link];
  if (href) element.href = href;
}
