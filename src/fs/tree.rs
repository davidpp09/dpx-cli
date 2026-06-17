//! Árbol del proyecto + mapa de símbolos (repo-map ligero, sin parser ni C).
//! Da al modelo QUÉ existe y QUÉ define cada archivo, para que lea solo lo
//! necesario en vez de adivinar o leer todo. Extraído de `fs`.

use std::fs;
use std::path::{Path, PathBuf};

/// Árbol de archivos del proyecto (para que el mentor sepa qué existe).
pub fn project_tree(root: &Path) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    tree_walk(root, "", 0, &mut out, &mut count);
    if out.is_empty() {
        out.push_str("(proyecto vacío)\n");
    }
    out
}

fn tree_walk(dir: &Path, prefix: &str, depth: usize, out: &mut String, count: &mut usize) {
    // Profundo a propósito: los paquetes Java anidan mucho (src/main/java/com/app/…),
    // y si cortamos pronto el modelo no ve los archivos .java reales.
    const MAX_DEPTH: usize = 10;
    const MAX_ENTRIES: usize = 300;
    if depth > MAX_DEPTH {
        return;
    }
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(e) => e.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches!(name.as_str(), "target" | "node_modules" | "build" | "dist") {
            continue;
        }
        *count += 1;
        if *count > MAX_ENTRIES {
            out.push_str(&format!("{prefix}… (truncado)\n"));
            return;
        }
        let is_dir = entry.path().is_dir();
        out.push_str(&format!("{prefix}{name}{}\n", if is_dir { "/" } else { "" }));
        if is_dir {
            tree_walk(&entry.path(), &format!("{prefix}  "), depth + 1, out, count);
        }
    }
}

// ── Mapa de símbolos (repo-map ligero, sin parser ni C) ─────────────
//
// Da al modelo un índice de QUÉ define cada archivo (fn/struct/class/def…) para
// que lea solo los que necesita, en vez de adivinar o leer todo. Heurística por
// línea (no es un parser completo: favorece precisión sobre exhaustividad).

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Rust,
    Python,
    JsTs,
    JavaKt,
    Other,
}

fn lang_of(path: &Path) -> Lang {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => Lang::Rust,
        Some("py") => Lang::Python,
        Some("ts" | "tsx" | "js" | "jsx" | "mjs") => Lang::JsTs,
        Some("java" | "kt" | "kts") => Lang::JavaKt,
        _ => Lang::Other,
    }
}

/// Mapa de símbolos del proyecto: por cada archivo fuente reconocido, sus
/// declaraciones de alto nivel. Acotado (archivos, símbolos por archivo y total
/// de líneas) para no inflar el prompt.
pub fn symbol_map(root: &Path) -> String {
    const MAX_FILES: usize = 60;
    const MAX_SYMS_PER_FILE: usize = 30;
    const MAX_TOTAL_LINES: usize = 220;

    let mut files: Vec<PathBuf> = Vec::new();
    collect_source_files(root, 0, &mut files);
    files.sort();

    let mut out = String::new();
    let mut total = 0usize;
    for path in files.iter().take(MAX_FILES) {
        if total >= MAX_TOTAL_LINES {
            break;
        }
        let Ok(content) = fs::read_to_string(path) else { continue };
        let syms = extract_symbols(&content, lang_of(path));
        if syms.is_empty() {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        out.push_str(&rel.display().to_string().replace('\\', "/"));
        out.push('\n');
        total += 1;
        for s in syms.iter().take(MAX_SYMS_PER_FILE) {
            if total >= MAX_TOTAL_LINES {
                break;
            }
            out.push_str("  ");
            out.push_str(s);
            out.push('\n');
            total += 1;
        }
    }
    out
}

fn collect_source_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    const MAX_DEPTH: usize = 10;
    if depth > MAX_DEPTH || out.len() > 500 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches!(name.as_str(), "target" | "node_modules" | "build" | "dist") {
            continue;
        }
        let p = entry.path();
        if p.is_dir() {
            collect_source_files(&p, depth + 1, out);
        } else if lang_of(&p) != Lang::Other {
            out.push(p);
        }
    }
}

fn extract_symbols(content: &str, lang: Lang) -> Vec<String> {
    match lang {
        Lang::Rust => rust_symbols(content),
        Lang::Python => python_symbols(content),
        Lang::JsTs => jsts_symbols(content),
        Lang::JavaKt => javakt_symbols(content),
        Lang::Other => Vec::new(),
    }
}

/// Primer identificador (`[A-Za-z0-9_]+`) al inicio de `s`, ignorando espacios.
fn first_ident(s: &str) -> String {
    s.trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Quita repetidamente cualquiera de `prefixes` (seguido de espacio) del inicio.
fn strip_kw_prefixes<'a>(mut s: &'a str, prefixes: &[&str]) -> &'a str {
    loop {
        let mut changed = false;
        for p in prefixes {
            if let Some(rest) = s.strip_prefix(p)
                && rest.starts_with(char::is_whitespace) {
                    s = rest.trim_start();
                    changed = true;
                    break;
                }
        }
        if !changed {
            return s;
        }
    }
}

fn rust_symbols(content: &str) -> Vec<String> {
    const MODS: &[&str] = &[
        "pub(crate)", "pub(super)", "pub(self)", "pub", "async", "unsafe", "const", "default",
        "extern",
    ];
    let mut out = Vec::new();
    for raw in content.lines() {
        let t = raw.trim_start();
        if t.starts_with("//") || t.starts_with('#') {
            continue;
        }
        let t = strip_kw_prefixes(t, MODS);
        if let Some(rest) = t.strip_prefix("impl ") {
            let head = rest.split('{').next().unwrap_or("").trim();
            if !head.is_empty() {
                out.push(format!("impl {head}"));
            }
            continue;
        }
        for kw in ["fn ", "struct ", "enum ", "trait ", "mod ", "macro_rules! "] {
            if let Some(rest) = t.strip_prefix(kw) {
                let name = first_ident(rest);
                if !name.is_empty() {
                    out.push(format!("{kw}{name}"));
                }
                break;
            }
        }
    }
    out
}

fn python_symbols(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let nested = raw.starts_with(' ') || raw.starts_with('\t');
        let t = raw.trim_start();
        let t = t.strip_prefix("async ").unwrap_or(t);
        if let Some(rest) = t.strip_prefix("def ") {
            let name = first_ident(rest);
            if !name.is_empty() {
                out.push(format!("{}def {name}", if nested { "  " } else { "" }));
            }
        } else if let Some(rest) = t.strip_prefix("class ") {
            let name = first_ident(rest);
            if !name.is_empty() {
                out.push(format!("class {name}"));
            }
        }
    }
    out
}

fn jsts_symbols(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in content.lines() {
        let t = raw.trim_start();
        let t = t.strip_prefix("export ").unwrap_or(t);
        let t = t.strip_prefix("default ").unwrap_or(t);
        let t = t.strip_prefix("async ").unwrap_or(t);
        if let Some(rest) = t.strip_prefix("function ") {
            let name = first_ident(rest.trim_start_matches('*').trim_start());
            if !name.is_empty() {
                out.push(format!("function {name}"));
            }
        } else {
            for kw in ["class ", "interface ", "enum ", "type "] {
                if let Some(rest) = t.strip_prefix(kw) {
                    let name = first_ident(rest);
                    if !name.is_empty() {
                        out.push(format!("{kw}{name}"));
                    }
                    break;
                }
            }
            // Funciones flecha: `const NAME = (...) =>` / `= async (...) =>`.
            for kw in ["const ", "let "] {
                if let Some(rest) = t.strip_prefix(kw) {
                    if (t.contains("=>") || t.contains("= (") || t.contains("=(")) && t.contains('=')
                    {
                        let name = first_ident(rest);
                        if !name.is_empty() {
                            out.push(format!("const {name}"));
                        }
                    }
                    break;
                }
            }
        }
    }
    out
}

fn javakt_symbols(content: &str) -> Vec<String> {
    const TYPES: &[&str] = &["class ", "interface ", "enum ", "record ", "object "];
    let mut out = Vec::new();
    for raw in content.lines() {
        let t = raw.trim();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            continue;
        }
        // Tipos (tras quitar modificadores que pueden precederlos).
        let stripped = strip_kw_prefixes(
            t,
            &["public", "private", "protected", "abstract", "final", "static", "sealed", "open", "data"],
        );
        let mut matched = false;
        for kw in TYPES {
            if let Some(rest) = stripped.strip_prefix(kw) {
                let name = first_ident(rest);
                if !name.is_empty() {
                    out.push(format!("{}{name}", kw.trim_end()));
                    matched = true;
                }
                break;
            }
        }
        if matched {
            continue;
        }
        // Kotlin: `fun NAME`.
        if let Some(rest) = stripped.strip_prefix("fun ") {
            let name = first_ident(rest);
            if !name.is_empty() {
                out.push(format!("fun {name}"));
                continue;
            }
        }
        // Métodos Java: con visibilidad, paréntesis y apertura de cuerpo `{`.
        let has_vis = ["public", "private", "protected"].iter().any(|v| t.starts_with(v));
        if has_vis && t.contains('(') && t.contains(')') && t.ends_with('{') {
            let sig = t.split('{').next().unwrap_or("").trim();
            if !sig.is_empty() {
                out.push(sig.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_symbols_extrae_lo_principal_e_ignora_comentarios() {
        let src = "\
// fn comentado no cuenta
use std::fmt;
pub fn arranca() {}
async fn corre() {}
pub(crate) struct Config { x: u8 }
enum Brain { A, B }
trait Mentor {}
impl Mentor for Config {}
    fn metodo_indentado() {}
";
        let s = rust_symbols(src);
        assert!(s.contains(&"fn arranca".to_string()));
        assert!(s.contains(&"fn corre".to_string()));
        assert!(s.contains(&"struct Config".to_string()));
        assert!(s.contains(&"enum Brain".to_string()));
        assert!(s.contains(&"trait Mentor".to_string()));
        assert!(s.iter().any(|x| x.starts_with("impl Mentor for Config")));
        assert!(s.contains(&"fn metodo_indentado".to_string()));
        assert!(!s.iter().any(|x| x.contains("comentado")));
    }

    #[test]
    fn python_y_jsts_symbols() {
        let py = "import os\nclass Foo:\n    def bar(self):\n        pass\nasync def baz():\n    pass\n";
        let p = python_symbols(py);
        assert!(p.contains(&"class Foo".to_string()));
        assert!(p.iter().any(|x| x.trim() == "def bar"));
        assert!(p.contains(&"def baz".to_string()));

        let js = "export function run() {}\nclass Widget {}\nexport const make = (x) => x*2;\ninterface Opts {}\n";
        let j = jsts_symbols(js);
        assert!(j.contains(&"function run".to_string()));
        assert!(j.contains(&"class Widget".to_string()));
        assert!(j.contains(&"const make".to_string()));
        assert!(j.contains(&"interface Opts".to_string()));
    }

    #[test]
    fn symbol_map_lista_archivos_y_simbolos_e_ignora_ruido() {
        let dir = std::env::temp_dir().join(format!("dpx-symmap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::create_dir_all(dir.join("target")).unwrap(); // debe ignorarse
        fs::write(dir.join("src/lib.rs"), "pub fn hola() {}\nstruct Estado {}\n").unwrap();
        fs::write(dir.join("target/gen.rs"), "fn no_deberia_aparecer() {}\n").unwrap();
        fs::write(dir.join("README.md"), "# nada de símbolos\n").unwrap();

        let map = symbol_map(&dir);
        assert!(map.contains("src/lib.rs"));
        assert!(map.contains("fn hola"));
        assert!(map.contains("struct Estado"));
        assert!(!map.contains("no_deberia_aparecer"), "target/ debe ignorarse");
        assert!(!map.contains("README"), "archivos sin símbolos no se listan");

        fs::remove_dir_all(&dir).unwrap();
    }
}
