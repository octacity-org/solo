import * as native from "@solo/native"
import { appInfo } from "./app.js"

/** A stored credential entry. */
export interface CredentialEntry {
  /** The account or username associated with the secret. */
  username: string
  /** The stored password or token. */
  secret: string
}

function resolveAppId(appId?: string): string {
  if (typeof appId === "string" && appId.trim().length > 0) {
    return appId.trim()
  }
  const identifier = appInfo().identifier
  if (!identifier) {
    throw new Error(
      'Credential operations need the app identifier. Set "solo": { "identifier": "com.example.app" } in package.json or pass an explicit appId',
    )
  }
  return identifier
}

function validateUrl(url: unknown): asserts url is string {
  if (typeof url !== "string" || url.trim().length === 0) {
    throw new TypeError("Credential URL must be a non-empty string")
  }
}

function validateUsername(username: unknown): asserts username is string {
  if (typeof username !== "string" || username.trim().length === 0) {
    throw new TypeError("Credential username must be a non-empty string")
  }
}

function normalizeSecret(secret: unknown): string {
  if (typeof secret === "string") {
    if (secret.length === 0) {
      throw new TypeError("Credential secret must not be empty")
    }
    return secret
  }
  if (secret instanceof Uint8Array) {
    if (secret.byteLength === 0) {
      throw new TypeError("Credential secret must not be empty")
    }
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(secret)
    } catch {
      throw new TypeError("Credential secret contains invalid UTF-8 data")
    }
  }
  throw new TypeError("Credential secret must be a string or Uint8Array")
}

/**
 * Store a secret securely in the operating system's credential vault.
 *
 * - Linux: stored in the Secret Service (GNOME Keyring / KWallet) with the app's
 *   `solo.identifier`, preventing cross-app or Zed conflicts (Issue #128).
 * - macOS: stored in the macOS Keychain under the app's `solo.identifier`
 *   (Issue #43).
 * - Windows: stored in the Windows Credential Manager (Issue #144).
 *
 * Rejects with a clear error if the prompt is dismissed, no Secret Service
 * is available on Linux, or on unsupported platforms. Secrets are never logged.
 */
export async function writeCredential(
  url: string,
  username: string,
  secret: string | Uint8Array,
  appId?: string,
): Promise<void> {
  validateUrl(url)
  validateUsername(username)
  const secretStr = normalizeSecret(secret)
  const resolvedAppId = resolveAppId(appId)
  await native.writeCredentials(url, username, secretStr, resolvedAppId)
}

/**
 * Retrieve a secret for `url` from the operating system's credential vault.
 * Returns `null` if no credential matches the URL.
 */
export async function readCredential(
  url: string,
  appId?: string,
): Promise<CredentialEntry | null> {
  validateUrl(url)
  const resolvedAppId = resolveAppId(appId)
  const result = await native.readCredentials(url, resolvedAppId)
  if (!result) return null
  return {
    username: result.username,
    secret: result.secret,
  }
}

/**
 * Remove a stored secret for `url` from the operating system's credential vault.
 * Returns `true` if an entry was removed, `false` if none was found.
 */
export async function deleteCredential(
  url: string,
  appId?: string,
): Promise<boolean> {
  validateUrl(url)
  const resolvedAppId = resolveAppId(appId)
  return await native.deleteCredentials(url, resolvedAppId)
}

/**
 * Secure credential storage namespace, matching Issue #43 and #128.
 */
export const credentials = {
  write: writeCredential,
  read: readCredential,
  delete: deleteCredential,
}
