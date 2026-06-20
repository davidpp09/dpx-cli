#!/usr/bin/env node
// Lanzador: encuentra el binario nativo (descargado por install.js) y le pasa
// los argumentos tal cual, heredando stdin/stdout/stderr para que el TUI de dpx
// funcione igual que ejecutando el .exe directo.

"use strict";

const { spawnSync } = require("child_process");
const path = require("path");
const fs = require("fs");

const exe = path.join(__dirname, "dpx.exe");

if (!fs.existsSync(exe)) {
  console.error("[@dpx-cli/dpx] Falta el binario. Reinstala con: npm i -g @dpx-cli/dpx");
  process.exit(1);
}

const result = spawnSync(exe, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`[@dpx-cli/dpx] No pude ejecutar dpx: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status === null ? 1 : result.status);
