# One verified account core for creators and app users

Status: accepted, 2026-11-06. Amends [ADR-0002](0002-kernel-owns-identity-and-visibility.md).

ADR-0002 put identity in the Kernel for app users. The launch plan admits creators the Kernel did not invite and strangers to their apps, so an address someone typed can no longer stand for a person. Both kinds of account need proof of email, recovery and suspension, and an export must carry them to a machine with no identity provider.

## Decision

The Kernel issues creator accounts and app accounts with one implementation, `kernel-rs/src/auth.rs`, in two scopes: `creator` in `owners.sqlite` and one scope per Deployment in its `identity.sqlite`. No external provider is needed to sign in or to run an installation. The hosted identity adapter stays optional and keeps its own accounts.

An account is pending, active or suspended, and only an active account gets a session. Session checks join on the state, so suspension ends live sessions without a sweep, and delegated MCP credentials and OAuth code exchanges read the creator's state on every use.

A challenge carries a purpose (verification, invitation, recovery) and a scope. The Kernel keeps its SHA-256 hash, an expiry and a consumed time. One transaction finds the challenge for that purpose and scope, marks it consumed, sets the password, activates the account and issues the session, so two concurrent answers produce one activation. Opening a link with GET changes nothing.

Verification requires choosing the password on the page the link opens. Signup in required mode takes only an email and stores no password. The plan allowed a second option, binding verification to the signup attempt and its browser. We did not take it because a link opened on a phone, or by an agent acting for the person, would then fail, and because it keeps two passwords in play: the one typed at signup and the one the verifier proves. With one password chosen after proof, a password a third party typed for someone else's address cannot survive, and verification, recovery and the coming invitation acceptance share one completion step: consume a token, set a password, enter.

`LITTLEBOX_ACCOUNT_VERIFICATION` chooses the mode for the installation. `local`, the default, keeps unverified signup so development without email works. `required` moves every account that never verified to pending, revokes its sessions, and keeps IDs and rows. The store records the mode it last ran under, so the move happens once per switch and again after a later local period.

## Considered options

- **Bind verification to the attempt and the browser.** Rejected for the reasons above.
- **Make verification required with no local mode.** Rejected because every development installation and test Kernel would need a mail transport before anyone could sign in.
- **Mark existing accounts verified during migration.** Rejected because nobody proved those addresses; an Allowlist only checked the text of an email.
- **Keep creator accounts in a separate store with their own code.** Rejected because the two scopes need the same states, links and refusals, and two copies would drift.

## Consequences

Required mode needs a mail transport; without one, signup and recovery answer 503 and existing active accounts still sign in. The transport is a seam (`AccountMail` in `kernel-rs/src/accounts.rs`); the only implementation today writes a development mailbox file. A full copy drops sessions and challenges, and an import deletes any it finds, so an old link never works on another machine. The operator suspends and resumes local creators with `littlebox-kernel creator-account-suspend` and `creator-account-resume`. Invitation acceptance and creator invitation codes plug into the same challenge model in later deliveries.

Note, 2026-11-06: account email now leaves through a persistent queue in `mail.sqlite`, recorded in [ADR-0012](0012-account-email-goes-through-a-persistent-queue.md). `AccountMail` stays as the transport seam the development mailbox and tests use. The identity store gained `request_signup_with` and `request_recovery_with`, which take a `ChallengeOutbox` that writes in the challenge's transaction.
