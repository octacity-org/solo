// Packaged Keychain round trip (package-commands.ts credentials). Each launch
// uses a fresh URL: an item left by an earlier build belongs to a binary with
// another signature, and touching it would raise a Keychain access prompt.
// The secrets are compared here and never shown.
import { createSignal } from "solid-js"
import { credentials, render, Text } from "@solo/solid"

const appId = "dev.solo.credentials-test"
const url = `https://solo.test/credentials/${process.pid}-${Date.now()}`
const secret = "s3cret-ü-🔑"

async function roundTrip(): Promise<string> {
  const steps: string[] = []
  try {
    steps.push(`missing=${await credentials.read(url, appId)}`)
    await credentials.write(url, "alice", secret, appId)
    const written = await credentials.read(url, appId)
    steps.push(`written=${written?.username}:${written?.secret === secret}`)
    await credentials.write(url, "bob", new TextEncoder().encode(`${secret}-2`), appId)
    const replaced = await credentials.read(url, appId)
    steps.push(`replaced=${replaced?.username}:${replaced?.secret === `${secret}-2`}`)
    steps.push(`isolated=${await credentials.read(url, `${appId}.other`)}`)
    steps.push(`deleted=${await credentials.delete(url, appId)}`)
    steps.push(`gone=${await credentials.read(url, appId)}`)
    steps.push(`again=${await credentials.delete(url, appId)}`)
  } catch (error) {
    steps.push(`error=${(error as Error).message}`)
    await credentials.delete(url, appId).catch(() => {})
  }
  return steps.join(" ")
}

function App() {
  const [result, setResult] = createSignal("pending")
  void roundTrip().then(setResult)
  return <Text testId="result">{result()}</Text>
}

render(() => <App />, { title: "Solo Credentials" })
