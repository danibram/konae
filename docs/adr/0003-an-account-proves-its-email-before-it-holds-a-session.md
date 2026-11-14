# An account proves its email before it holds a session

Status: accepted.

Konae admits owners it did not invite, and apps admit strangers, so an address someone typed cannot stand for a person. Owner accounts and app accounts both need proof of email, recovery and suspension, and an app account must reach another machine with no identity provider behind it.

## Decision

### One account core for owners and app users

`kernel-rs/kernel/src/auth.rs` issues every account in a scope. Konae's owners have one scope in `owners.sqlite`, and each Deployment has its own in the app's `data/identity.sqlite`. No outside provider is needed to sign in or to run an installation. Konae may still accept owners from Clerk, which keeps its own accounts.

- An account is pending, active or suspended. Only an active account gets a session. A session check joins on the state, so a suspension ends live sessions without a sweep, and Konae reads an owner's state on every use of a delegated token.
- An account holds at most 20 open sessions. A sign-in past that ends the oldest, so one account cannot fill its app's identity file.
- A challenge has a purpose (verification, invitation or recovery) and a scope. The Kernel keeps its SHA-256 hash, an expiry and a consumed time. One transaction consumes the challenge, sets the password, activates the account and issues the session, so two answers to one link activate it once. Opening a link with GET changes nothing.
- The person chooses the password on the page the verification link opens. Signup in required mode takes only an email and stores no password, so a password a third party typed for someone else's address cannot survive verification.
- `KONAE_ACCOUNT_VERIFICATION` chooses the mode. `local`, the default, keeps unverified signup so development without email works. `required` moves every account that never verified to pending and revokes its sessions, and records the mode so the move happens once per switch.
- A failed sign-in in the browser that started a signup for that email adds a hint to open the emailed link first. The hint depends only on a cookie that every signup request sets, whatever the address's state, so it tells nobody whether an address has an account.

### Invitations

An invitation is an `invitation` challenge on a pending account, answered by the same step as verification and recovery. Inviting a new email creates the pending account at once, so two answers race on one row. An active account is told to sign in instead, and a suspended one stays suspended. The invitation's history lives beside the accounts, and an export clears its token hashes, so a pending invitation shows `needs-resend` at the destination.

Closed refuses what a person does alone, which is a signup and the verification that completes it. An invitation is the owner choosing one person, so Open, Invitation only and Closed all accept it. An Allowlist still has to list the address. Closing signup does not end pending invitations. The owner revokes the ones that should end.

Listing accounts, suspending them, inviting and changing the Signup Policy need `manage_users`. `signup_policy_set` is the one way to change the policy of an app that already runs, so a credential that may only publish cannot widen who joins.

### Owners are admitted by Konae

Konae admits owners with invitation codes that the operator issues, lists and revokes with `konae creator-code-create`, `creator-code-list` and `creator-code-revoke`. A code is stored as a hash and is consumed in the transaction that creates the pending owner. The account it creates still verifies its email. The installation's setup key creates an owner only while no owner and no code exists. A code presented for a taken email is consumed and creates nothing, and without a valid code a taken email and a new one get the same answer.

### An app account may sign in with the app's own Google client

- Each app has its own Google client. The owner creates a Web application client in their own Google Cloud console and sets it as the secrets `google_client_id` and `google_client_secret` with `use: "google"`. The installation has no Google client. `sign_in_methods_set` refuses `google` until both halves are set, and `secret_delete` refuses either half while Google is on.
- The whole flow stays on the app's host. The owner registers `/_konae/auth/google/callback` on the generated hostname, on a verified custom domain and, while the path is on, on `<domain>/p/<id>`. `sign_in_methods_read` returns these as `redirectUris`, and `start` refuses a host that is not among them. The home address offers no Google sign-in.
- The Kernel fetches Google's keys only from `https://www.googleapis.com/oauth2/v3/certs`, never from a URL a token names. The token must name Google as issuer, the app's client as audience, an unexpired time, the flow's nonce and a verified email.
- A Google sign-in reaches the app account that holds the same email, compared without case. Google proves an address only where it is the authority, which is an `@gmail.com` address or one whose domain the token's `hd` claim names. Any other address signs in with email and password.
- Under local verification anyone can create an active account for an address they do not hold. A Google sign-in for that address replaces its password with an unusable one, ends its sessions and marks the email proven.
- The Signup Policy still decides whether an email without an account gets one. Password sign-in stays on beside Google, so every account keeps a way back in.
- `signInMethods` travels in `konae.json`. The client's secrets do not, so on another installation the app offers email and password, and `sign_in_methods_read` says so, until the owner sets the client there and registers that host.

## Rejected options

- **Bind verification to the signup attempt and its browser.** A link opened on a phone, or by an agent acting for the person, would fail, and two passwords would be in play.
- **Required verification with no local mode.** Every development installation and test Kernel would need a mail transport before anyone could sign in.
- **Mark existing accounts verified when the mode changes.** Nobody proved those addresses. An Allowlist checks only the text of an email.
- **Separate code for owner accounts.** The two scopes need the same states, links and refusals, and two copies would drift.
- **End pending invitations when the policy moves to Closed.** People would hold links that stop working without a word.
- **One Google client for the installation.** The consent screen would name the operator, not the app, and an app that moves would need an operator to register its hosts.
- **Link identities by Google's subject.** An app moved to another client would lose every link, and people with a password account would have to connect Google first.
- **Accept any `email_verified` address from Google.** A Google account made with an outside address only proves that someone once read mail there.

## Consequences

- Required mode needs a mail transport (ADR-0012). Without one, signup and recovery answer 503 and active accounts still sign in.
- An export drops sessions and open links, so an old link never works on another machine.
- An account's `verified_at` may come from a challenge or from Google.

On 2026-11-14 the commands became `owner-code-create`, `owner-code-list`, `owner-code-revoke`, `owner-account-suspend`, `owner-account-resume` and `owner-suspend`, because every other page calls this person an owner. The old names answer with the new one and fail.
