// Build a real standalone .app for both regression automation and manual UI QA.
import { execFileSync } from "node:child_process"
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { build } from "vite"
import { solidUniversal } from "../../vite.js"

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..")
const fixture = process.argv[2] ?? "application-commands"
if (!["application-commands", "application-shortcuts", "desktop", "system-appearance", "file-dialogs", "background-lifecycle", "context-menus", "multi-window", "automation-frames"].includes(fixture)) throw new Error("Unknown app fixture")
const name = fixture === "context-menus" ? "Menus" : fixture === "desktop" ? "Desktop" : fixture === "system-appearance" ? "Appearance" : fixture === "file-dialogs" ? "Dialogs" : fixture === "background-lifecycle" ? "Background" : fixture === "multi-window" ? "Windows" : fixture === "automation-frames" ? "Frames" : "Commands"
// CI runs Solid tests before its package build. Build the public package here
// so this fixture verifies the same exports that a consuming app bundles.
execFileSync("bun", ["run", "build"], { cwd: root, stdio: "inherit" })
const output = mkdtempSync(join(tmpdir(), "solo-commands-"))
const bundle = join(output, `Solo ${name}.app`)
const executable = join(bundle, "Contents", "MacOS", `Solo${name}`)
mkdirSync(dirname(executable), { recursive: true })
if (fixture === "context-menus") {
  const resources = join(bundle, "Contents", "Resources")
  mkdirSync(resources, { recursive: true })
  const helper = `solo-context-menu.darwin-${process.arch}`
  copyFileSync(resolve(root, "../native", helper), join(resources, helper))
  chmodSync(join(resources, helper), 0o755)
}
if (fixture === "background-lifecycle") {
  const resources = join(bundle, "Contents", "Resources")
  mkdirSync(resources, { recursive: true })
  writeFileSync(join(resources, "menu.png"), Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=", "base64"))
}
mkdirSync(join(output, "node_modules", "@solo"), { recursive: true })
symlinkSync(root, join(output, "node_modules", "@solo", "solid"), "dir")
await build({
  root, configFile: false, plugins: [solidUniversal()],
  ssr: { noExternal: true },
  build: { ssr: `src/__tests__/fixtures/${fixture}.tsx`, outDir: join(output, "js"), target: "node20" },
})
// The bootstrap runs the app in a Worker, which Bun embeds only as an entrypoint.
execFileSync("bun", ["build", join(output, "js", `${fixture}.js`), join(output, "js", `${fixture}.app.js`), "--compile", "--outfile", executable], { cwd: output, stdio: "inherit" })
chmodSync(executable, 0o755)
writeFileSync(join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>Solo${name}</string>
<key>CFBundleIdentifier</key><string>dev.solo.${name.toLowerCase()}-test</string>
<key>CFBundleName</key><string>Solo ${name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>`)
execFileSync("/usr/bin/codesign", ["--force", "--deep", "--sign", "-", bundle], { stdio: "inherit" })
console.log(`commands-executable:${executable}`)
