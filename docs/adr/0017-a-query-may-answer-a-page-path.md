# A query may answer a page path

Status: proposed.

A blog keeps its posts in rows, and each post needs an address a person can share. Today a page file answers one path, so a post is either `/post.html` with the slug in the query string or a page in `spa` routing that reads the path in the browser. Either way the HTML that leaves the Kernel is the same for every post. A chat app that builds a card reads that HTML without running its script, so every post shows the same card or none.

## Decision

- An anonymous query may declare `"page": { "path": "/posts/:slug", "file": "post.html", "list": "posts.published" }`. The app chooses the shape of the path. `/blog/:year/:slug` and `/people/:id` are the same declaration.
- A path has segments of the characters an HTTP route allows (ADR-0013). A segment that starts with `:` is a parameter, the first segment is never one, and the parameters are exactly the query's input bindings. `kernel-rs/kernel/src/page-cases.json` holds the rules, and the Validation Gate and the Kernel both run it.
- For a `GET`, an HTTP route wins, then a file of the artifact, then a page path, then the routing mode's answer for a missing path. An action declares `http` or `page`, not both.
- The Kernel binds each parameter by its binding's type and runs the query. With no row it answers 404 with the app's `404.html` or its own page. With a row it serves `file` and writes the row's `title`, `description` and `image` into `<head>`. The query must return a column named `title`.
- The page's script reads the parameters from `miniapp.page.params`, on a hostname, a custom domain and a path address alike.
- At build, the builder prefixes relative URLs in a page file with enough `../` segments to resolve from the declared page path back to the app root. The Kernel serves the artifact unchanged. The builder leaves root-relative URLs as written and warns because they leave a path address. An app script builds app links with `miniapp.page.url(path)`, which keeps the active path-address prefix.
- `list` names an anonymous query with no input whose columns are the parameters, with an optional `updated`. Its rows are the addresses the sitemap lists (ADR-0016), up to the 1000 rows a query returns.
- The declaration belongs to the `actions` module (ADR-0015) and works with `seo` off: the page gets its title, and the response carries `noindex`. With `seo` on, the Kernel adds the card and the canonical address from the row.
- A page path takes one of the Deployment's statement slots and the query budget. It does not take the per-address limit of anonymous actions, which would answer a crawler 429. Usage counts it as a page view.
- A page answered from a row carries `Cache-Control: no-cache` and a validator made from the release and the row's values, because a row changes without a release.
- The editor preview remains one `srcDoc` document, so it cannot resolve page paths or populate `miniapp.page.params`. It writes no `<head>` tags for row values (see [named divergences](../divergences.md#what-the-preview-lacks)).

## Rejected options

- **`/post?slug=hello`.** It works today with no new rule, because the Kernel ignores the query string of a file. The address of a post is shared and indexed, and changing its shape later means a redirect for every post.
- **A file named for its parameter, such as `posts/[slug].html`.** The path would live in a file name and the query in `actions.json`, and both doors would have to check that the two agree.
- **A list of pages among the options of `seo`.** Which address a post has is the app's decision and travels with its files. It is not a setting of the installation.
- **The Kernel renders the body from the row.** That is a template language. The page's script already renders the body.
- **Write one page per row at publish.** A post written after the publish would have no address until the next one.

## Consequences

- A crawler that runs no script reads the title, the description and the card of a post, and not its body.
- Both parsers of `actions.json` learn `page`, the Kernel gains a path matcher between its routes and its files, and the SDK gains `miniapp.page`.
- A sitemap holds at most 1000 addresses for each page declaration.
- The builder rewrites the relative URLs in a page file to account for the depth of its declared page path. It does not add a `<base>` tag, which the app's Content Security Policy refuses (ADR-0014).
- A local probe sent 20 page requests to one Deployment. All returned 200 in 4.8 ms. A concurrent route call took 3.9 ms, compared with a 2.3 ms baseline call. The probe is small and does not set a capacity limit.
