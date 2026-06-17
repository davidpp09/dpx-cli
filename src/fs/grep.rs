//! Búsqueda de texto en el proyecto (`search_in_project`) y ORPHAN-SWEEP
//! (`orphan_refs`): referencias colgando al nombre de un módulo tras borrarlo.
//! Extraído de `fs`.

use std::path::Path;

use super::{cap_tail, run_command};

/// Busca una cadena o patrón (grep simple) en los archivos del proyecto.
pub fn search_in_project(cwd: &Path, pattern: &str) -> String {
    let cmd = if cwd.join(".git").exists() {
        format!("git grep -i -n \"{}\" -- \":!target\" \":!node_modules\" \":!build\"", pattern)
    } else if cfg!(windows) {
        format!("findstr /s /i /n /c:\"{}\" *.java *.xml *.yml *.properties *.rs *.md *.toml", pattern)
    } else {
        format!("grep -r -i -n \"{}\" --exclude-dir={{target,node_modules,build,.git}} .", pattern)
    };

    let out = run_command(cwd, &cmd);
    cap_tail(&out, 100)
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
