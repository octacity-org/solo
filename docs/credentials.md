# Secure Credential Storage

Solo apps can store and retrieve sensitive credentials (tokens, passwords, API keys) securely using the host operating system's native credential store.

- **Linux:** Backed by the **Secret Service API** (GNOME Keyring, KWallet) over D-Bus via the `oo7` crate ([#128](https://github.com/octacity-org/solo/issues/128)). Credentials are partitioned by the application's `solo.identifier` so that separate apps and Zed credentials never conflict.
- **macOS:** Backed by the **macOS Keychain** ([#43](https://github.com/octacity-org/solo/issues/43)). Credentials are internet passwords whose server is `"solo:<app_id>:<url>"`, so separate apps and Zed credentials never conflict.
- **Windows:** Backed by the **Windows Credential Manager** ([#144](https://github.com/octacity-org/solo/issues/144)) using `CredWriteW`, `CredReadW`, and `CredDeleteW`. Credentials are keyed as `"solo:<app_id>:<url>"` to prevent collisions across applications.

```tsx
import { credentials } from "@solo/solid"

// Store a secret:
await credentials.write("https://api.github.com", "octacat", "ghp_secretToken123")

// Retrieve a secret:
const entry = await credentials.read("https://api.github.com")
if (entry) {
  console.log(`Signed in as ${entry.username}`)
}

// Remove a secret when signing out:
const deleted = await credentials.delete("https://api.github.com")
```

## API Reference

| Method | Returns | Description |
|---|---|---|
| `credentials.write(url, username, secret, appId?)` | `Promise<void>` | Stores a secret keyed by URL and username. Replaces existing entries for the same URL in this app. |
| `credentials.read(url, appId?)` | `Promise<CredentialEntry \| null>` | Reads `{ username, secret }` for the given URL, or returns `null` if none was found. |
| `credentials.delete(url, appId?)` | `Promise<boolean>` | Deletes the credential for the given URL. Returns `true` if an entry was removed, `false` otherwise. |

Top-level functions `writeCredential`, `readCredential`, and `deleteCredential` are also exported directly from `@solo/solid`.

### Arguments

- **`url`** (`string`): The non-empty URL or service identifier (e.g. `"https://api.github.com"`).
- **`username`** (`string`): The user account identifier (e.g. `"octacat"`).
- **`secret`** (`string | Uint8Array`): The password or token to store. If passed as a `Uint8Array`, it is decoded as UTF-8.
- **`appId`** (`string`, optional): The reverse-DNS identifier used to isolate stored credentials. When omitted, it defaults to `solo.identifier` in `package.json`. If neither is provided, credential operations reject with an error.

## Platform Differences & Partitioning

On Linux, credentials are saved with attributes:
- `solo-app`: `<app_id>`
- `url`: `<url>`
- `username`: `<username>`

And an item label of `"<app_id> (<url>)"`. This guarantees that:
- Two Solo apps storing credentials for the same service URL do not read or overwrite each other.
- Solo apps do not collide with Zed editor credentials (which use the hardcoded `zed-github-account` label).

On macOS, credentials are saved in the login keychain as internet passwords with server `"solo:<app_id>:<url>"` and account `<username>`. A keychain item trusts the app that created it, so another app, or a build of yours signed differently, can get the standard Keychain access prompt when it touches that item.

On Windows, credentials are saved in Windows Credential Manager as generic credentials keyed by `"solo:<app_id>:<url>"`. Windows Credential Manager limits a secret to 2,560 bytes (`CRED_MAX_CREDENTIAL_BLOB_SIZE`); longer secrets reject with an error.

## Error Handling & Security

- **Secrets are never logged** or included in error messages.
- If a keyring is locked, the desktop displays its standard unlock prompt. If the user dismisses the prompt, the operation rejects clearly with `"Keyring unlock prompt was dismissed"`.
- On macOS, denying the Keychain access prompt rejects `write()` and `delete()` with `"Keychain access was denied"`. `read()` resolves to `null` instead, because the system reports a denied read the same way as a missing item.
- On headless systems or environments without a Secret Service daemon on D-Bus, calls reject with `"No Secret Service provider available on the session bus"`.
- Missing credentials return `null` from `read()` rather than throwing.
