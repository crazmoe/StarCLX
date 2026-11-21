#!/usr/bin/env node
// Erzeugt apps/desktop/src-tauri/THIRD-PARTY-LICENSES.txt: die Lizenzen aller
// Fremdkomponenten, die in StarCLX stecken. Das Programm zeigt die Datei unter
// Einstellungen → Konto → Lizenz, die Pakete legen sie unter
// /usr/share/doc/starclx/ ab. Die CI erzeugt sie vor jedem Paketbau neu.
//
// Braucht: cargo-about (cargo install --locked cargo-about --features cli) und
// installierte npm-Abhängigkeiten (npm ci in apps/desktop).
//
//   node scripts/third-party-licenses.mjs

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const desktop = join(root, "apps/desktop");
const out = join(desktop, "src-tauri/THIRD-PARTY-LICENSES.txt");
const lic = (f) => readFileSync(join(root, "packaging/licenses", f), "utf8");
const line = "=".repeat(80);

// C-Bibliotheken, die fest in StarCLX einkompiliert sind
const native = [
  ["libre (Baresip Foundation)", "BSD-3-Clause", "https://github.com/baresip/re", lic("libre.txt")],
  ["baresip (Baresip Foundation)", "BSD-3-Clause", "https://github.com/baresip/baresip", lic("baresip.txt")],
  [
    "spandsp (Steve Underwood), nur G.722",
    "LGPL-2.1",
    "https://github.com/freeswitch/spandsp",
    "spandsp ist statisch eingebunden. Der vollständige Quelltext von StarCLX steht unter\n" +
      "https://github.com/crazmoe/StarCLX bereit; damit lässt sich StarCLX mit einer\n" +
      "geänderten spandsp-Version neu bauen (LGPL-2.1, Abschnitt 6).\n" +
      "spandsp is linked statically. The complete source of StarCLX is available at the\n" +
      "address above, so StarCLX can be rebuilt with a modified spandsp (LGPL-2.1, section 6).\n\n" +
      lic("LGPL-2.1.txt"),
  ],
];

// Oberfläche: Laufzeit-Abhängigkeiten plus Svelte (dessen Laufzeit steckt im
// gebauten JavaScript), jeweils mit ihren Abhängigkeiten
function npmPackages() {
  const pkg = JSON.parse(readFileSync(join(desktop, "package.json"), "utf8"));
  const todo = [...Object.keys(pkg.dependencies ?? {}), "svelte"];
  const seen = new Map();
  while (todo.length) {
    const name = todo.pop();
    if (seen.has(name)) continue;
    const dir = join(desktop, "node_modules", name);
    if (!existsSync(join(dir, "package.json"))) continue;
    const meta = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    const file = readdirSync(dir).find((f) => /^(licen[cs]e|copying)(\.(md|txt))?$/i.test(f));
    seen.set(name, {
      version: meta.version,
      license: typeof meta.license === "string" ? meta.license : (meta.license?.type ?? "siehe Text"),
      text: file ? readFileSync(join(dir, file), "utf8") : "",
    });
    todo.push(...Object.keys(meta.dependencies ?? {}));
  }
  return [...seen].sort(([a], [b]) => a.localeCompare(b));
}

const rust = execFileSync(
  "cargo",
  ["about", "generate", "--fail", "-c", join(root, "about.toml"), "-m", join(desktop, "src-tauri/Cargo.toml"), join(root, "about.hbs")],
  { cwd: root, encoding: "utf8", maxBuffer: 64 << 20 },
);

let text =
  "StarCLX – Lizenzen der verwendeten Komponenten / Third-party licences\n\n" +
  "StarCLX selbst steht unter der MIT-Lizenz (siehe LICENSE). Es enthält die folgenden\n" +
  "Komponenten anderer Urheber unter deren Lizenzen.\n" +
  "StarCLX itself is MIT licensed. It contains the following third-party components.\n\n" +
  "Systembibliotheken wie WebKitGTK, GTK, OpenSSL, zlib, PipeWire, PulseAudio und ALSA\n" +
  "werden zur Laufzeit vom System geladen (im AppImage teils mitgeliefert) und stehen\n" +
  "unter ihren eigenen Lizenzen (u. a. LGPL-2.1, Apache-2.0, Zlib).\n\n";

text += `${line}\nC-BIBLIOTHEKEN / NATIVE LIBRARIES\n`;
for (const [name, id, url, body] of native) {
  text += `${line}\n${name} – ${id}\n${url}\n${"-".repeat(80)}\n${body.trim()}\n\n`;
}

text += `${line}\nOBERFLÄCHE / USER INTERFACE (npm)\n`;
for (const [name, p] of npmPackages()) {
  text += `${line}\n${name} ${p.version} – ${p.license}\n${"-".repeat(80)}\n${p.text.trim() || `Lizenz: ${p.license}`}\n\n`;
}

text += `${line}\nRUST-BIBLIOTHEKEN / RUST CRATES\n${rust.trim()}\n`;

writeFileSync(out, text.replace(/\r\n/g, "\n"));
console.log(`${out}: ${(text.length / 1024).toFixed(0)} KB`);
