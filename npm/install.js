// Postinstall: descarga el binario `dpx.exe` del GitHub Release que coincide con
// la versión de este paquete y lo deja en bin/. El paquete npm publicado NO trae
// el binario (se baja aquí), así pesa kilobytes y se versiona con el Release.
//
// REQUISITO: el Release `vX.Y.Z` debe existir y ser PÚBLICO en el repo de abajo.

"use strict";

const https = require("https");
const fs = require("fs");
const path = require("path");

const pkg = require("./package.json");
const VERSION = pkg.version;

// 👇 CAMBIA esto por tu repo de GitHub (owner/nombre) antes de publicar.
const REPO = "TU_USUARIO/dpx-cli";

const URL = `https://github.com/${REPO}/releases/download/v${VERSION}/dpx.exe`;

// Por ahora solo Windows x64 (lo declara también `os`/`cpu` en package.json).
if (process.platform !== "win32" || process.arch !== "x64") {
  console.error("[@dpx-cli/dpx] Por ahora solo Windows x64. Mac/Linux: próximamente.");
  process.exit(0); // no romper instalaciones en otras plataformas
}

const binDir = path.join(__dirname, "bin");
const dest = path.join(binDir, "dpx.exe");
fs.mkdirSync(binDir, { recursive: true });

function fail(msg) {
  console.error(`[@dpx-cli/dpx] No pude descargar el binario: ${msg}`);
  console.error(`  URL: ${URL}`);
  console.error(`  Verifica que el Release v${VERSION} exista y el repo sea público.`);
  process.exit(1);
}

// GitHub redirige las descargas de Release a un CDN: seguimos los redirects.
function download(url, redirects) {
  if (redirects > 5) return fail("demasiados redirects");
  https
    .get(url, { headers: { "User-Agent": "dpx-cli-installer" } }, (res) => {
      if ([301, 302, 303, 307, 308].includes(res.statusCode)) {
        return download(res.headers.location, redirects + 1);
      }
      if (res.statusCode !== 200) {
        return fail(`HTTP ${res.statusCode}`);
      }
      const out = fs.createWriteStream(dest);
      res.pipe(out);
      out.on("finish", () => out.close(() => console.log("[@dpx-cli/dpx] binario instalado ✓")));
      out.on("error", (e) => fail(e.message));
    })
    .on("error", (e) => fail(e.message));
}

download(URL, 0);
