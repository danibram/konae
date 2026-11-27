# Issue a PDF document

Documents renders a Typst PDF after a Server Action mutation commits. The issue record, its snapshot, its template, its resource text, its download query and the Typst version stay in the app database. The PDF uses the shared blob store, so normal export and import carry it with the app.

Follow [Turn on an app module](./guides/manage-modules.md#turn-on-an-app-module). Enable `actions` and `auth`, then enable `documents`. The operator must make Typst available before the app uses Documents.

Declare an authenticated mutation with one string input for the stable issue key. Issue documents through the app's Server Actions. Functions, Call continuations and File intake consumers refuse actions that declare a document issue. Name the columns its `RETURNING` clause must freeze, a Typst template, and the authenticated query that decides who can download the PDF.

```json
{
  "documents.issue": {
    "kind": "mutation",
    "access": "authenticated",
    "statement": "UPDATE invoices SET issued=1 WHERE id=? AND owner_id=?",
    "bindings": [
      {"source":"input","key":"id","type":"string"},
      {"source":"actor","field":"id"}
    ],
    "guard": "This invoice is unavailable.",
    "issue": {
      "key": "id",
      "download": "documents.read",
      "snapshot": ["id", "total"],
      "template": "#set page(paper: \"a4\")\n#text(issue.id)\n#text(str(issue.total))",
      "resources": {},
      "typstVersion": "0.14.2"
    }
  },
  "documents.read": {
    "kind": "query",
    "access": "authenticated",
    "statement": "SELECT id FROM invoices WHERE id=? AND owner_id=? LIMIT 1",
    "bindings": [
      {"source":"input","key":"issueKey","type":"string"},
      {"source":"actor","field":"id"}
    ]
  }
}
```

The Kernel adds the declared snapshot columns to its own `RETURNING` clause. The mutation must change exactly one row. Those returned values become the immutable `issue` object in the template. Resource values are UTF-8 text, or `base64:<standard-base64>` for binary assets such as fonts. The key accepts letters, digits, `-`, `_`, `.`, and `~`, up to 200 characters. Reusing a key with identical inputs keeps the existing issue. Reusing it with different inputs fails the mutation.

## Verify a PDF on the app

1. Put the two Actions inside the `actions` map of `app/actions.json` with `version: 1`. Use an invoice row owned by a test account.
2. Sign in as that account on `konae dev` or the published app, then invoke `documents.issue` through the app's Server Actions.
3. Open `/_konae/documents/<issue-key>` on the app's address. Include the app base path if it uses a path address. Wait for the render before checking for a `200` PDF.
4. Open the same URL as a different signed-in account. Expect `404` if the download query finds no row for that account.

The Kernel runs the pinned query under the app's shared database lock. A pending or missing issue also returns `404`.

## Renderer setup and limits

The Konae and standalone Kernel images include Typst 0.14.2. For a native installation, put that version of `typst` on the Kernel's `PATH` or set `KONAE_DOCUMENTS_TYPST_BIN` to its executable path. Startup checks the executable and version when a served app has Documents enabled. For `konae dev`, put Typst 0.14.2 on `PATH`. Run `typst --version` in the same environment that starts the Kernel.

Tokio admits two renders per installation and one per app. An app can queue up to 1,000 unfinished issues. Each render has a 30 second wall clock deadline and a 30 second CPU limit. The Kernel caps Typst memory at 1 GiB with an address-space limit on Linux and RSS polling on macOS. Each render also allows at most 2 MiB of frozen input, 64 KiB of renderer logs and a 10 MiB PDF. The app's shared Files per-file limit and total quota apply to the stored PDF. The Kernel supplies the render inputs in a private temporary folder and sets it as Typst's project root. The Kernel stores the template, resources, issue snapshot, named read query, release ID and Typst version with the issue.

Export the app folder and import it on another Kernel to move issued PDFs and pending renders. The destination needs the same Typst version before it can resume render work.
