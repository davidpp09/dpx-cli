//! Búsqueda de texto en el proyecto (`search_in_project`) y ORPHAN-SWEEP
//! (`orphan_refs`): referencias colgando al nombre de un módulo tras borrarlo.
//! Extraído de `fs`.

use std::fs;
use std::path::Path;

use super::run_command;

/// Directorios que NUNCA se exploran (build, dependencias, vcs, datos generados).
const SKIP_DIRS: &[&str] = &[
    "target", "node_modules", "build", "dist", ".git", ".dpx", ".fastembed_cache",
    ".venv", "venv", "__pycache__", ".idea", ".vscode", "vendor",
];

/// Tope de coincidencias para no inundar el contexto del modelo.
const MAX_HITS: usize = 100;

/// Busca un término en el proyecto. Soporta alternación con `|` (cualquiera de
/// las alternativas, como substring case-insensitive). Implementación NATIVA en
/// Rust: SIN shell, así que es inmune a las comillas, pipes y regex que rompían
/// `git grep`/`findstr` en Windows (la cmd.exe interpretaba `|` como pipe y
/// reventaba el comando — la causa de que el agente se atascara). Acota a
/// [`MAX_HITS`] coincidencias.
pub fn search_in_project(cwd: &Path, pattern: &str) -> String {
    let needles: Vec<String> = pattern
        .split('|')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if needles.is_empty() {
        return "[patrón de búsqueda vacío]".to_string();
    }

    let mut hits: Vec<String> = Vec::new();
    let mut truncated = false;
    walk_and_match(cwd, cwd, &needles, &mut hits, &mut truncated);

    if hits.is_empty() {
        return format!("[sin coincidencias para `{pattern}`]");
    }
    let mut out = hits.join("\n");
    if truncated {
        out.push_str(&format!(
            "\n[… cortado en {MAX_HITS} coincidencias; afina el patrón si necesitas más]"
        ));
    }
    out
}

/// Recorre el árbol (omitiendo [`SKIP_DIRS`] y carpetas ocultas), lee cada
/// archivo de TEXTO (los binarios fallan `read_to_string` y se omiten solos) y
/// acumula `ruta:línea:contenido` por cada línea que contenga alguna `needle`.
fn walk_and_match(
    root: &Path,
    dir: &Path,
    needles: &[String],
    hits: &mut Vec<String>,
    truncated: &mut bool,
) {
    if *truncated {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if *truncated {
            return;
        }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_and_match(root, &path, needles, hits, truncated);
        } else {
            let Ok(content) = fs::read_to_string(&path) else { continue };
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");
            for (i, line) in content.lines().enumerate() {
                let low = line.to_lowercase();
                if needles.iter().any(|n| low.contains(n)) {
                    hits.push(format!("{}:{}:{}", rel, i + 1, line.trim_end()));
                    if hits.len() >= MAX_HITS {
                        *truncated = true;
                        return;
                    }
                }
            }
        }
    }
}

/// Stems demasiado genéricos para el orphan-sweep: aparecen en medio proyecto,
/// así que avisar por ellos sería ruido constante. Mejor callar que dar falsos.
const GENERIC_STEMS: &[&str] = &[
    "mod", "lib", "main", "index", "types", "type", "utils", "util", "app", "config",
    "init", "test", "tests", "helpers", "helper", "common", "core", "api", "client",
    "server", "model", "models", "view", "views", "style", "styles", "router", "routes",
];

/// ¿Una línea de `git grep` es un match real (`ruta:NNN:contenido`)? Filtra
/// cabeceras (`--- stdout ---`, `exit code: N`) y posibles líneas de error.
fn looks_like_grep_hit(l: &str) -> bool {
    let mut parts = l.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_path), Some(num), Some(_rest)) => num.trim().parse::<u32>().is_ok(),
        _ => false,
    }
}

/// ¿El hit cae en un directorio de build/artefactos que no interesa? Como el
/// orphan-sweep usa `git grep --no-index` (no respeta `.gitignore`), filtramos
/// estas rutas a mano para no avisar por copias en target/node_modules/etc.
fn in_build_dir(hit: &str) -> bool {
    let path = hit.split(':').next().unwrap_or("").replace('\\', "/");
    const SKIP: &[&str] = &["target/", "node_modules/", "build/", "dist/", ".git/", ".dpx/"];
    SKIP.iter().any(|p| path.starts_with(p) || path.contains(&format!("/{p}")))
}

/// ORPHAN-SWEEP: tras borrar `deleted_rel`, busca referencias que quedaron
/// colgando a su nombre de módulo (el *stem* del archivo) como PALABRA completa,
/// para que el agente las limpie y no deje imports/usos rotos — el fallo nº1 de
/// dpx (borra/renombra y deja referencias). El archivo ya fue borrado, así que
/// no se cuenta a sí mismo. Usa `git grep --untracked` para ver también los
/// archivos que dpx acaba de escribir sin commitear. Devuelve las líneas
/// encontradas (acotadas), o `None` si el stem es genérico (ruido), no hay git,
/// o no quedó ninguna referencia.
pub fn orphan_refs(cwd: &Path, deleted_rel: &str) -> Option<String> {
    let stem = Path::new(deleted_rel).file_stem()?.to_str()?.to_string();
    if stem.len() < 3
        || stem.chars().any(|c| c.is_whitespace()) // evita líos de quoting en cmd
        || GENERIC_STEMS.contains(&stem.to_lowercase().as_str())
    {
        return None;
    }
    if !cwd.join(".git").exists() {
        return None; // el sweep determinista se apoya en git grep
    }
    // `--no-index` busca también lo NO trackeado (lo que dpx acaba de escribir
    // sin commitear) — `--untracked` resultó poco fiable entre versiones de git.
    // `-F` literal, `-w` palabra completa (recorta el ruido de subcadenas). No
    // respeta .gitignore, así que `in_build_dir` filtra target/node_modules/etc.
    let out = run_command(cwd, &format!("git grep --no-index -F -w -n {stem}"));
    let refs: Vec<&str> = out
        .lines()
        .filter(|l| looks_like_grep_hit(l) && !in_build_dir(l))
        .collect();
    if refs.is_empty() {
        return None;
    }
    Some(refs.into_iter().take(20).collect::<Vec<_>>().join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_nativo_encuentra_soporta_alternacion_y_omite_target() {
        let dir = std::env::temp_dir().join(format!("dpx-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn alpha() {}\nlet beta = 1;\n").unwrap();
        std::fs::write(dir.join("src/b.rs"), "fn gamma() {}\n").unwrap();
        // target/ debe omitirse (build artifact).
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("target/x.rs"), "fn alpha() {}\n").unwrap();

        // Alternación con `|` (lo que rompía con git grep/cmd.exe): halla alpha O gamma.
        let out = search_in_project(&dir, "fn alpha|fn gamma");
        assert!(out.contains("src/a.rs"), "debe hallar a.rs: {out}");
        assert!(out.contains("src/b.rs"), "debe hallar b.rs: {out}");
        assert!(!out.contains("target"), "NO debe buscar en target/: {out}");

        assert!(search_in_project(&dir, "noexiste_xyz").contains("sin coincidencias"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_refs_detecta_referencias_colgando() {
        let dir = std::env::temp_dir().join(format!("dpx-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Repo git mínimo (el sweep se apoya en git grep --untracked).
        run_command(&dir, "git init");
        std::fs::write(dir.join("app.rs"), "mod widget;\nfn main() { widget::run(); }\n").unwrap();
        std::fs::write(dir.join("readme.md"), "nada que ver aquí\n").unwrap();
        // Tras 'borrar' widget.rs, su módulo sigue referenciado en app.rs.
        let refs = orphan_refs(&dir, "src/widget.rs").expect("debería detectar la referencia colgando");
        assert!(refs.contains("app.rs"), "el orphan-sweep debe señalar app.rs, vi: {refs}");
        // Un stem genérico NO dispara el sweep (sería puro ruido).
        assert!(orphan_refs(&dir, "src/mod.rs").is_none());
        // Un módulo sin referencias tampoco avisa.
        assert!(orphan_refs(&dir, "src/inexistente_xyz.rs").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
