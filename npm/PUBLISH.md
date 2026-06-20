# Publicar @dpx-cli/dpx en npm

dpx es un binario de Rust; npm solo distribuye un envoltorio que descarga el
`.exe` del GitHub Release. Por eso hay DOS publicaciones por versión: el Release
de GitHub (el binario) y el paquete npm (el envoltorio).

## Una sola vez (setup)

1. **Sube el repo a GitHub** y hazlo **público** (el postinstall descarga el
   binario sin auth; si es privado, no funciona sin token).
2. **Reemplaza `TU_USUARIO/dpx-cli`** por tu repo real en:
   - `npm/package.json` → `repository.url`
   - `npm/install.js` → la constante `REPO`
   - `npm/README.md` → el enlace de GitHub
3. **Crea la organización `dpx-cli`** en npmjs.com (gratis) y haz `npm login`.

## Cada release

1. **Sube la versión en los DOS sitios a la vez** (mismo `X.Y.Z`):
   - `Cargo.toml` → `version = "X.Y.Z"`
   - `npm/package.json` → `"version": "X.Y.Z"`
2. **Tag + push** → dispara el workflow que compila y sube el binario:
   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
   Espera a que el Action `Release` termine y el Release `vX.Y.Z` tenga `dpx.exe`.
3. **Publica el paquete npm**:
   ```bash
   cd npm
   npm publish --access public
   ```

## Probar como usuario

```bash
npm install -g @dpx-cli/dpx
dpx
```

## Notas

- Por ahora solo **Windows x64** (`os`/`cpu` en package.json). En otras
  plataformas `npm install` avisa que no está soportado.
- Para añadir Mac/Linux: agrega esos targets al workflow `release.yml` (subiendo
  `dpx-macos`, `dpx-linux`) y amplía `install.js` para elegir por `process.platform`.
