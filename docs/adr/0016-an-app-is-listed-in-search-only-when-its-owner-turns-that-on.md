# An app is listed in search only when its owner turns that on

Status: proposed.

A crawler gets an app's built files as they are. The same page answers on the generated hostname, the custom domain, `/p/<id>` and `/p/<name>`, with and without `.html`, and nothing says which address is the real one. A private app answers 200 with its sign-in page on every path, `/robots.txt` included. A link shared in a chat shows no card, because no page says its own address or image. Public means that whoever has the link may open the app. It does not mean the owner asked to be in a search engine.

## Decision

### Off means not listed

- `seo` is a module (ADR-0015). While it is off, and on every private app, each response carries `X-Robots-Tag: noindex`.
- The Kernel's own pages carry `noindex` whatever the module says: the sign-in page, its 404 page and the maintenance page.
- A private app answers `/robots.txt` as `text/plain` with no rule that keeps a crawler out, not with the sign-in page. A crawler that a `Disallow` stops never reads the `noindex`, and a search engine can then list a linked address by its URL alone.
- The upgrade leaves `seo` off for every app, so a public app that a search engine lists today leaves it until its owner turns the module on.

### The owner chooses the address a crawler is told

- The option `canonical` is `"domain"`, `"hostname"` or `"path"`. Without it the Kernel uses the custom domain once it is verified, and the generated hostname otherwise.
- For the installation home, `"path"` means `<domain>/`.
- When the chosen address does not exist on this installation, the Kernel falls back to the default, and `modules_read` says which address it uses and why.
- Every other address keeps answering. Each page there names the chosen address, and nothing redirects (ADR-0014).

### The Kernel writes what depends on the address

- For each HTML page it serves, the Kernel adds to `<head>` the canonical link, `og:url`, `og:title`, `og:description`, `og:image`, `og:type` and `twitter:card`. It takes the title and description from the page's own `<title>` and description, and the image from the option `image`, the path of an image in the published artifact.
- It leaves alone any of those tags the page already has, and it escapes every value it writes.
- The canonical URL is the chosen address and the page's path without `.html`, with `/` for `index.html` and no query string.
- A page without a `<title>` is served with `noindex` and left out of the sitemap. `modules_read` lists it. No publish is refused for it.
- In `spa` routing, the answer for a path the artifact does not hold carries `noindex`.
- The Kernel answers `/robots.txt` when the artifact holds none, and `/sitemap.xml` when the artifact holds none, each on its own. The sitemap lists every listed page with its absolute canonical URL. The Kernel's `robots.txt` names the sitemap. When an app's `robots.txt` has no `Sitemap:` line, the Kernel adds one that points to `/sitemap.xml` at the canonical address.
- The validator of a page changes with the release, the module's options and the chosen address.

### The page says what it is about

- The title and the description are the app's. The builder moves every `<meta>`, `<link>` and JSON-LD script a fragment opens with into `<head>`. Today it moves only the title and the icons, so a description written in a fragment stays in `<body>`.

## Rejected options

- **A `Link` header in place of the tags.** Google reads the header. No Bing document says Bing does, and a chat app builds its card from the HTML alone.
- **Tags written at build time.** The builder does not know the address, an export would carry the old one, and each app folder holds its own copy of the builder.
- **Redirect every other address to the chosen one.** A session belongs to one host, and routes and API keys already answer on the generated hostname.
- **List a public app unless its owner says no.** A person who shares a form with ten people has not asked for it to be found.
- **Refuse a publish when a page has no title.** The owner would learn about search listing by failing to publish something else.
- **One `robots.txt` the Kernel writes for the bare domain, with every app on a path address in it.** An app that wants its own `robots.txt` takes its generated hostname or a custom domain, which is a host of its own.
- **Upload the image in the settings.** It needs a new place for bytes. The image is a file of the app.

## Consequences

- An app on a path address, other than the home, has no `robots.txt` of its own. The header is its control, and its sitemap at `/p/<name>/sitemap.xml` reaches a search engine only when its owner submits it.
- The editor preview writes none of this. `konae dev` does, with the local host as the address, because it has no domain.
- The Kernel changes HTML it serves for the first time. It adds tags to `<head>` and nothing else.
- The docs site renders its pages in the browser, and the home swaps to Spanish there. Each needs its content in the HTML it ships before listing helps it.
- An app in `spa` routing lists one page.
