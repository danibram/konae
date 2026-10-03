# The Littlebox Kernel owns identity and deployment visibility

Status: accepted, 2026-10-21. Amends no earlier ADR. [ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md) amends it.

Authentication, sessions, grants, public/private visibility and action authorization are product semantics owned by the provider-independent Littlebox Kernel. Cloudflare, Railway and self-hosted infrastructure may supply storage and execution adapters, but their identity or access products are not the source of truth and must not change Kernel decisions. This keeps deployments portable and gives local, managed and bring-your-own-infrastructure installations the same security model, at the cost of operating our own session and authorization layer.

## Update, 2026-10-23: signup policy and anonymous actions

Visibility now pairs with a per-Deployment signup policy that the Kernel enforces when an account is created: Open (anyone), Allowlist (exact emails or `@domain` entries) or Closed (no new accounts). Public Deployments start Open and private ones start Closed. A private Deployment requires a signed-in app account before its shell loads. On a public Deployment, the Kernel lets a signed-out visitor call only the actions declared `anonymous`. The owner sets the policy in the editor's Auth panel or through admin MCP tools. Both decisions stay in the Kernel, as this ADR requires.

## Note, 2026-11-07: ADR-0011 amends this ADR

[ADR-0011](0011-one-verified-account-core-for-creators-and-app-users.md) records two amendments. Creator accounts now live in the Kernel too, verified by email, so no identity provider is needed to sign in or to run an installation. HTTP routes of actions declared `anonymous` answer to the callers each route takes, a signature, an API key or a session, on a private Deployment as well; the shell and the runtime endpoint of a private Deployment still need a login. Its notes also add Invitation only to the signup policy above and let an owner's invitation be accepted under Closed.
