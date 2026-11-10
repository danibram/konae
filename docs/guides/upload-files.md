# Upload files

Use `miniapp.files` for the bytes and a mutation for the row that remembers the file's ID.

## How it works

The Kernel stores small files for one Deployment, in its file store. A file is private to the person who uploaded it unless the upload asks for `"public"`. Large bytes do not belong in a SQLite column. Keep the file's ID in a row and let the Kernel keep the bytes.

- 1 byte to 2 MiB per file, and 20 MiB of files per Deployment. An operator can change both with `KONAE_FILE_MAX_BYTES` and `KONAE_FILES_QUOTA_BYTES`, and each refusal names the number in force.
- An upload needs an app session. A signed-out visitor on a public Deployment can list and read public files only.
- Only the uploader changes a file's visibility or deletes it. A deleted file stops counting against the quota.

## Steps

1. Add a table that holds the file's ID. This is from the starter app.

    ```sql
    CREATE TABLE IF NOT EXISTS attachments (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id TEXT NOT NULL,
      file_id TEXT NOT NULL,
      name TEXT NOT NULL,
      size INTEGER NOT NULL CHECK (size >= 0),
      public INTEGER NOT NULL DEFAULT 0 CHECK (public IN (0, 1)),
      created_at INTEGER NOT NULL
    );
    ```

2. Add a mutation that stores the ID next to the actor.

    ```json
    "attachments.add": {
      "kind": "mutation",
      "access": "authenticated",
      "statement": "INSERT INTO attachments (user_id, file_id, name, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
      "bindings": [
        { "source": "actor", "field": "id" },
        { "source": "input", "key": "fileId", "type": "string", "maxLength": 64 },
        { "source": "input", "key": "name", "type": "string", "maxLength": 180 },
        { "source": "input", "key": "size", "type": "number" },
        { "source": "kernel", "field": "now" }
      ]
    }
    ```

3. Upload from the page, then record the file. Attach the listener in JavaScript, because a published app refuses inline handler attributes.

    ```js
    const stored = await miniapp.files.upload(file);   // private to the signed-in user
    await miniapp.actions.call("attachments.add", { fileId: stored.id, name: stored.name, size: stored.size });
    ```

    Pass `{ visibility: "public" }` only when every visitor of a public app may read the bytes.
4. Show a file with `files.url`. It returns the path `/_konae/files/<id>` on the app's own origin, which `img`, `fetch` and a link all reach. A Blob URL from `files.list` or `files.upload` cannot be fetched again under the page's policy.

    ```js
    image.src = await miniapp.files.url(photo.file_id);
    ```

5. Publish or remove a file later with the uploader's session. The ID and the `files.url` path stay the same.

    ```js
    await miniapp.files.update(draft.id, { visibility: "public" });
    await miniapp.files.delete(note.file_id);   // { id, deleted: true }
    ```

6. If the row's mutation fails after the upload, delete the file so it does not keep counting against the quota.

    ```js
    try {
      await miniapp.actions.call("attachments.add", { fileId: stored.id, name: stored.name, size: stored.size });
    } catch (error) {
      await miniapp.files.delete(stored.id).catch(() => {});
      throw error;
    }
    ```

## Refusals you may meet

- `authentication-required` when nobody is signed in.
- `Files must be between 1 byte and 2 MiB.` when the file is empty or too large.
- `This deployment has reached its 20 MiB file quota.`
- `Only the person who uploaded this file may change or delete it.`

## Check it worked

- After `files_upload`, `files_update` or `files_delete`, read the files back with `files_list`. In the page, that is `miniapp.files.list()`.
- Open the `files.url` path in the app and check the file loads. A visitor without a session on a private Deployment gets 401 `authentication-required`. Anyone else who may not read the file gets 404 `File not found.`
- Query the row from step 2 to confirm the ID was stored.

The editor preview keeps files in browser storage, refuses a file over 2 MiB and never reaches the 20 MiB quota. A published app sends files to the Kernel in chunked, checksummed transfers, so try a real upload on `konae dev` or a Deployment. An export carries the files when the app moves.

## Read next

- [Add a Server Action](./add-a-server-action.md)
- [Take the app elsewhere](./take-the-app-elsewhere.md)
- [Limits](../limits.md)
- [Examples](../examples.md)
