# Issue a PDF document

Documents renders a Typst PDF after a Server Action mutation commits. The issue record, its snapshot, its template, its resource text, its download query and the Typst version stay in the app database. The PDF uses the shared blob store, so normal export and import carry it with the app.

Turn on `documents`, `actions` and `auth` in the app's module settings. Files and Functions can stay off. The Documents module requires Actions and Auth.

Declare an authenticated mutation with one string input for the stable issue key. Issue documents through the app's Server Actions. Functions, Call continuations and File intake consumers refuse actions that declare a document issue. Name the columns its `RETURNING` clause must freeze, a Typst template, and the authenticated query that decides who can download the PDF.

```json
{
  "documents.issue": {
    "kind": "mutation",
    "access": "authenticated",
    "statement": "UPDATE invoices SET issued=1 WHERE id=?",
    "bindings": [{"source":"input","key":"id","type":"string"}],
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

The Kernel serves a ready PDF at `/_konae/documents/<issue-key>`. It runs the pinned authenticated query for the current account while holding the app's shared database lock. A query that returns no row gets the same `404` as a missing issue. The app's access rule decides which accounts can read a row.

## Renderer setup and limits

The Konae and standalone Kernel images include Typst 0.14.2. For a native installation, put that version of `typst` on the Kernel's `PATH` or set `KONAE_DOCUMENTS_TYPST_BIN` to its executable path. Startup checks the executable and version when a served app has Documents enabled.

Tokio admits two renders per installation and one per app. An app can queue up to 1,000 unfinished issues. Each render has a 30 second wall clock deadline and a 30 second CPU limit. The Kernel caps Typst memory at 1 GiB with an address-space limit on Linux and RSS polling on macOS. Each render also allows at most 2 MiB of frozen input, 64 KiB of renderer logs and a 10 MiB PDF. The app's shared Files per-file limit and total quota apply to the stored PDF. The Kernel supplies the render inputs in a private temporary folder and sets it as Typst's project root. The Kernel stores the template, resources, issue snapshot, named read query, release ID and Typst version with the issue.

Export the app folder and import it on another Kernel to move issued PDFs and pending renders. The destination needs the same Typst version before it can resume render work.
