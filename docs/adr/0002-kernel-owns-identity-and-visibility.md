# The Littlebox Kernel owns identity and deployment visibility

Authentication, sessions, grants, public/private visibility and action authorization are product semantics owned by the provider-independent Littlebox Kernel. Cloudflare, Railway and self-hosted infrastructure may supply storage and execution adapters, but their identity or access products are not the source of truth and must not change Kernel decisions. This keeps deployments portable and gives local, managed and bring-your-own-infrastructure installations the same security model, at the cost of operating our own session and authorization layer.

## Update, 2026-10-23: signup policy and anonymous actions

Visibility now pairs with a per-Deployment signup policy that the Kernel enforces when an account is created: Open (anyone), Allowlist (exact emails or `@domain` entries) or Closed (no new accounts). Public Deployments start Open and private ones start Closed. A private Deployment requires a signed-in app account before its shell loads. On a public Deployment, the Kernel lets a signed-out visitor call only the actions declared `anonymous`. The owner sets the policy in the editor's Auth panel or through admin MCP tools. Both decisions stay in the Kernel, as this ADR requires.
