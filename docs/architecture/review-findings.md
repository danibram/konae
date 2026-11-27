# Follow-ups from the architecture review

These notes record observations from the development checkout. The proposed checks below give a maintainer a way to close each item.

| Observation | Evidence | Check for a fix |
| --- | --- | --- |
| The Club replay stops before its first invitation. | On the development checkout the first private release returned signup policy `closed`. The fixture expected `invitation`. This run did not reach the organizer or transfer checks. | Reconcile the intended first-release policy and the fixture, then run the [Club replay](../../examples/showcase/club/README.md) through import on its second Kernel. |
| The [product-fit replay](../../examples/product-fit/README.md) stops at the board sign-in title. | All eight examples published and loaded in Chrome. The waitlist and private journal checks passed. The board assertion expected `Sign in` and the page title was `Sign in · board`. | Assert the intended sign-in behavior and rerun the remaining checks. This result alone says nothing about the board's role checks. |
| The two action parsers report different first errors. | Reproduced with 101 actions, one missing `bindings`. The editor parser reports the count limit and the Kernel reports the missing field. | Feed the same source to both paths and compare code, message and hint. |
| A public folder with an unknown, explicitly disabled module fails to open. | Reproduced on the local Kernel with two copies of the same static app. Only the copy containing `"future-documents":{"on":false}` failed. This is a compatibility limitation of the current format. | On versions supporting the proposed rule, preserve the off marker through export and reject an unknown required capability before activation. |
| Static bytes and the Deployment record can come from different revisions during publication. | The publish path switches `dist/` before replacing the record in memory. This is a code observation. | Interleave a public GET with publish and rollback in a fixture. Check body and ETag together in both outcomes. |
| Repeated MCP action calls parse the same declarations. A CSS-only publish with Actions on copies the database during preparation. | Both operations are visible in the implementation. Their latency and CPU costs have not been measured. | Count parses per warm call and bytes copied per code-only release. Compare latency against the same fixtures before changing either path. |

Documents and FTP have proposed storage and recovery requirements. The [architecture](../architecture.md#who-owns-which-store) records their acceptance cases. They describe features still to build.
