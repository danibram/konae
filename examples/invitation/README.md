# Invitation requests

The page where a visitor asks for an invitation code. An installation that admits owners by invitation gives its home somewhere to send people who have no code. It is a public Konae app with one table, one anonymous Server Action and no accounts, in English and Spanish.

The home links to it at `<domain>/p/invitation/`.

## What it does

- `requests.create` inserts the email, what the person wants to build and the page's language.
- Its `guard` refuses an address that already asked, and a value that is not an email address.
- Its `notify` sends one email to the app's contact email for each new row. A first publish from Konae fills the contact email with the owner's. `contact_email_set` changes it.
- The privacy and terms links show only when `<domain>/p/politica-de-privacidad/` and `<domain>/p/terminos-y-condiciones/` answer.

The app sends no code itself. The operator reads the requests and issues each code with `konae creator-code-create`, as [Operate an installation](../../docs/operate.md#creator-admission) describes.

## Read the requests

In the editor, open the project's database view. From a folder with a credential:

```sh
bun run konae -- sql --dir /tmp/konae-invitation --prod "SELECT email, building, lang, created_at FROM requests ORDER BY created_at DESC LIMIT 50"
```

## Files

| File | Role |
| --- | --- |
| `index.html` | The page. |
| `schema.sql`, `actions.json` | The `requests` table and `requests.create`. |
| `invitation.js` | Sends the form, holds the Spanish text and points the links at this installation from `location`. |
| `invitation.css` | The few rules of this page. |
| `styles.css` | The design system in the editor's variant, a copy of the home's. |

## Publish it

Run these from the repository root, with an owner account on the installation.

```sh
KONAE_EMAIL=you@example.com KONAE_PASSWORD=... bun run konae -- login --dir /tmp/konae-invitation --url https://app.<domain>
bun run konae -- init --dir /tmp/konae-invitation --name "Invitation"
rm -rf /tmp/konae-invitation/app && cp -R examples/invitation /tmp/konae-invitation/app
```

Before the first publish, set `visibility` to `"public"` and `signupPolicy` to `{ "mode": "closed" }` in `/tmp/konae-invitation/konae.json`. Then publish and give it the path name:

```sh
bun run konae -- publish --dir /tmp/konae-invitation
```

```text
path_set  { deploymentId, enabled: true, name: "invitation" }
path_read { deploymentId }
```

The app has no accounts, so `path_set` needs no `acceptSharedOrigin`.
