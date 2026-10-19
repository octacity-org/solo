/**
 * Solo WebView Preview
 *
 * Dogfood for `<webview>`: a native pane on the left (the part an app like
 * Newsprint keeps fully native) and a real WKWebView on the right, with the
 * three lifecycle events driving native status text.
 *
 * `<webview>` is macOS-only. Off macOS the element renders nothing and the
 * native side logs "Unknown element type: webview" each frame.
 *
 * Run:
 *   bun run build && bun dist/index.js   (or: bun run start)
 */

import { createSignal } from "solid-js"
import { render, View, Text, Button } from "@solo/solid"

const ARTICLES = [
  { id: "solo", title: "Solo", url: "https://example.com/solo" },
  { id: "gpui", title: "GPUI", url: "https://example.com/gpui" },
] as const

function Preview() {
  const [url, setUrl] = createSignal<string>(ARTICLES[0].url)
  const [status, setStatus] = createSignal("idle")

  return (
    <View
      style={{
        display: "flex",
        flexDirection: "row",
        width: "100%",
        height: "100%",
        backgroundColor: "#11111b",
      }}
    >
      {/* ── Native pane: never touches WebKit ─────────────────────────── */}
      <View
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 8,
          width: 220,
          padding: 16,
          backgroundColor: "#1e1e2e",
        }}
      >
        <Text
          testId="heading"
          style={{ fontSize: 16, fontWeight: "bold", color: "#cdd6f4" }}
        >
          Articles
        </Text>
        {ARTICLES.map((article) => (
          <Button
            testId={`open-${article.id}`}
            onClick={() => setUrl(article.url)}
            style={{
              backgroundColor: url() === article.url ? "#89b4fa" : "#313244",
              padding: 10,
            }}
          >
            <Text
              style={{
                fontSize: 14,
                fontWeight: "bold",
                color: url() === article.url ? "#11111b" : "#bac2de",
              }}
            >
              {article.title}
            </Text>
          </Button>
        ))}
      </View>

      {/* ── Web pane ─────────────────────────────────────────────────── */}
      <View
        style={{
          display: "flex",
          flexDirection: "column",
          flexGrow: 1,
          // Flex items cannot shrink below content without an explicit zero
          // minimum; the webview has no intrinsic size, so this is safe.
          minWidth: 0,
          minHeight: 0,
        }}
      >
        <webview
          testId="preview"
          url={url()}
          userAgent="SoloWebViewPreview/1.0"
          onNavigation={(event) => setStatus(`navigating: ${event.value ?? ""}`)}
          onLoad={(event) => setStatus(`loaded: ${event.value ?? ""}`)}
          onLoadError={(event) => setStatus(`failed: ${event.value ?? ""}`)}
          style={{ flexGrow: 1, minHeight: 0 }}
        />
        <Text
          testId="status"
          style={{
            fontSize: 12,
            padding: 8,
            color: "#a6adc8",
            backgroundColor: "#181825",
          }}
        >
          {status()}
        </Text>
      </View>
    </View>
  )
}

render(() => <Preview />, {
  title: "Solo WebView Preview",
  width: 1000,
  height: 700,
})
