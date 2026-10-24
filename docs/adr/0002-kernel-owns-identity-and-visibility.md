# The Littlebox Kernel owns identity and deployment visibility

Status: accepted, 2026-10-21. Amends no earlier ADR. [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md) amends it.

Authentication, sessions, grants, public/private visibility and action authorization are product semantics owned by the provider-independent Littlebox Kernel. Cloudflare, Railway and self-hosted infrastructure may supply storage and execution adapters, but their identity or access products are not the source of truth and must not change Kernel decisions. This keeps deployments portable and gives local, managed and bring-your-own-infrastructure installations the same security model, at the cost of operating our own session and authorization layer.

## Update, 2026-10-23: signup policy and anonymous actions

Visibility now pairs with a per-Deployment signup policy that the Kernel enforces when an account is created: Open (anyone), Allowlist (exact emails or `@domain` entries) or Closed (no new accounts). Public Deployments start Open and private ones start Closed. A private Deployment requires a signed-in app account before its shell loads. On a public Deployment, the Kernel lets a signed-out visitor call only the actions declared `anonymous`. The owner sets the policy in the editor's Auth panel or through admin MCP tools. Both decisions stay in the Kernel, as this ADR requires.

## Note, 2026-11-07: ADR-0011 amends this ADR

[ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md) records two amendments. Creator accounts now live in the Kernel too, verified by email, so no identity provider is needed to sign in or to run an installation. HTTP routes of actions declared `anonymous` answer to the callers each route takes, a signature, an API key or a session, on a private Deployment as well; the shell and the runtime endpoint of a private Deployment still need a login. Its notes also add Invitation only to the signup policy above and let an owner's invitation be accepted under Closed.

## Update, 2026-11-09: visibility fails closed

A Deployment is public only when its manifest says `"public"`. A record with no visibility, or with any other value, is private: its shell, its assets and its runtime need a login, and its default Signup Policy is Closed. The editor's own reading of a manifest follows the same rule. A new Project is private in the editor too, as it already was through MCP and `littlebox init`, except when it is born as one of the templates made for visitors without an app account: the shared list, the public form and the file gallery. The publish dialog proposes whatever the manifest says, so a first release is private unless the owner or the template chose public. We chose this over keeping public as the starting point because an owner who publishes without reading the dialog should expose nothing they did not ask to expose. The cost is that a Deployment record written without a visibility, if one exists, now asks for a login until its owner republishes it as public.
