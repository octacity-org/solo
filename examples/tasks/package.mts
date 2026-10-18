// Package the tasks app as a standalone macOS .app and a compressed .dmg.
//
//   bun package.mts   →  dist/Tasks.app, dist/Tasks.dmg
//
// The .app embeds Bun and the native addon (~78 MB). The .dmg uses ULMO
// (LZMA), the smallest format macOS mounts natively (10.15+): ~23 MB versus
// ~29 MB as a zip and ~33 MB as hdiutil's default UDZO.
import { execFileSync } from "node:child_process"
import { mkdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

if (process.platform !== "darwin") throw new Error("package.mts builds a macOS .app")

const root = dirname(fileURLToPath(import.meta.url))
const dist = join(root, "dist")
const name = "Tasks"
const bundle = join(dist, `${name}.app`)
const executable = join(bundle, "Contents", "MacOS", name)
const dmg = join(dist, `${name}.dmg`)

const run = (command: string, args: string[]) =>
  execFileSync(command, args, { cwd: root, stdio: "inherit" })
const mb = (path: string) => `${(statSync(path).size / 1024 / 1024).toFixed(1)} MB`

console.log("[package] vite build")
run("bun", ["run", "build"])

console.log("[package] bun build --compile")
mkdirSync(dirname(executable), { recursive: true })
run("bun", ["build", "dist/index.js", "--compile", "--minify", "--outfile", executable])
writeFileSync(join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "https://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>${name}</string>
<key>CFBundleIdentifier</key><string>dev.solo.tasks</string>
<key>CFBundleName</key><string>${name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>`)
run("/usr/bin/codesign", ["--force", "--deep", "--sign", "-", bundle])
console.log(`[package] ${bundle} (executable ${mb(executable)})`)

console.log("[package] hdiutil create -format ULMO")
const staging = join(dist, "dmg")
rmSync(staging, { recursive: true, force: true })
mkdirSync(staging)
run("ditto", [bundle, join(staging, `${name}.app`)])
symlinkSync("/Applications", join(staging, "Applications"))
run("hdiutil", ["create", "-quiet", "-volname", name, "-srcfolder", staging, "-format", "ULMO", "-ov", dmg])
rmSync(staging, { recursive: true, force: true })
console.log(`[package] ${dmg} (${mb(dmg)})`)
