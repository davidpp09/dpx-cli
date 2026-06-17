//! Traducción de respuestas LSP a ediciones/ubicaciones concretas: aplicar un
//! `WorkspaceEdit` (rename) como reescrituras de archivo, convertir posiciones
//! UTF-16 a offsets de byte, localizar un símbolo en una línea y formatear
//! referencias. Extraído de `lsp`.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

/// ¿El `WorkspaceEdit` trae cambios reales (en cualquiera de los dos formatos)?
pub(super) fn workspace_edit_has_changes(edit: &Value) -> bool {
    edit.get("documentChanges").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
        || edit.get("changes").and_then(Value::as_object).is_some_and(|m| !m.is_empty())
}

/// Traduce un `WorkspaceEdit` LSP a reescrituras de archivo completas. Soporta
/// los dos formatos (`documentChanges` —preferido por rust-analyzer— y `changes`)
/// y junta notas para las operaciones de recurso (renombrar/crear/borrar archivo)
/// que NO aplicamos automáticamente.
pub(super) fn apply_workspace_edit(
    cwd: &Path,
    edit: &Value,
) -> Result<(Vec<crate::fs::FileWrite>, Vec<String>)> {
    let mut writes = Vec::new();
    let mut notes = Vec::new();
    if let Some(dcs) = edit.get("documentChanges").and_then(Value::as_array) {
        for dc in dcs {
            // Las operaciones de recurso traen `kind` (rename/create/delete).
            if let Some(kind) = dc.get("kind").and_then(Value::as_str) {
                notes.push(format!(
                    "[rename_symbol: el language server sugiere además una operación de archivo \
                     `{kind}` que dpx no aplica sola; hazla a mano si hace falta]"
                ));
                continue;
            }
            if let (Some(uri), Some(edits)) =
                (dc["textDocument"]["uri"].as_str(), dc["edits"].as_array())
            {
                writes.push(build_renamed_write(cwd, uri, edits)?);
            }
        }
    } else if let Some(map) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in map {
            if let Some(edits) = edits.as_array() {
                writes.push(build_renamed_write(cwd, uri, edits)?);
            }
        }
    }
    Ok((writes, notes))
}

/// Lee un archivo, aplica sus `TextEdit`s y devuelve la reescritura completa.
fn build_renamed_write(cwd: &Path, uri: &str, edits: &[Value]) -> Result<crate::fs::FileWrite> {
    let rel = uri_to_rel(cwd, uri).ok_or_else(|| anyhow!("URI no relativizable: {uri}"))?;
    let content = std::fs::read_to_string(cwd.join(&rel))
        .with_context(|| format!("no pude leer {rel} para el rename"))?;
    let new_content = apply_text_edits(&content, edits)?;
    Ok(crate::fs::FileWrite { path: rel, content: new_content })
}

/// Aplica una lista de `TextEdit` LSP a `content`. Convierte cada posición
/// (línea, carácter UTF-16) a offset de bytes y aplica los splices de ATRÁS
/// hacia DELANTE, para que los offsets aún sin tocar no se corran.
fn apply_text_edits(content: &str, edits: &[Value]) -> Result<String> {
    let pos = |e: &Value, which: &str| -> Option<(u32, u32)> {
        let l = e["range"][which]["line"].as_u64()? as u32;
        let c = e["range"][which]["character"].as_u64()? as u32;
        Some((l, c))
    };
    let mut spans: Vec<(usize, usize, String)> = Vec::with_capacity(edits.len());
    for e in edits {
        let (sl, sc) = pos(e, "start").ok_or_else(|| anyhow!("TextEdit sin range.start"))?;
        let (el, ec) = pos(e, "end").ok_or_else(|| anyhow!("TextEdit sin range.end"))?;
        let start = position_to_byte(content, sl, sc)
            .ok_or_else(|| anyhow!("posición start ({sl},{sc}) fuera de rango"))?;
        let end = position_to_byte(content, el, ec)
            .ok_or_else(|| anyhow!("posición end ({el},{ec}) fuera de rango"))?;
        let new_text = e["newText"].as_str().unwrap_or("").to_string();
        spans.push((start, end, new_text));
    }
    spans.sort_by_key(|s| std::cmp::Reverse(s.0)); // descendente por inicio
    let mut out = content.to_string();
    for (start, end, new_text) in spans {
        if start > end || end > out.len() {
            return Err(anyhow!("rango de edición inválido ({start}..{end} en {} bytes)", out.len()));
        }
        out.replace_range(start..end, &new_text);
    }
    Ok(out)
}

/// Convierte una posición LSP `(line0, char_utf16)` (0-based; carácter en
/// unidades UTF-16) a offset de BYTES en `content`. Clampa al fin de línea si el
/// carácter la rebasa, y al fin de archivo si la línea lo hace.
fn position_to_byte(content: &str, line0: u32, char_utf16: u32) -> Option<usize> {
    let mut offset = 0usize;
    let mut cur = 0u32;
    for seg in content.split_inclusive('\n') {
        if cur == line0 {
            let text = seg.strip_suffix('\n').unwrap_or(seg);
            let text = text.strip_suffix('\r').unwrap_or(text);
            let mut u16c = 0u32;
            for (b, ch) in text.char_indices() {
                if u16c >= char_utf16 {
                    return Some(offset + b);
                }
                u16c += ch.len_utf16() as u32;
            }
            return Some(offset + text.len()); // carácter rebasa la línea → fin del texto
        }
        offset += seg.len();
        cur += 1;
    }
    (cur == line0).then_some(content.len()) // línea una más allá de la última = fin del archivo
}

/// Localiza un símbolo dentro de una línea (1-based) de `text` y devuelve su
/// posición LSP `(line0, character0)` 0-based, donde el carácter se cuenta en
/// unidades UTF-16 (lo que exige el protocolo). `None` si la línea no existe o
/// el símbolo no aparece en ella.
pub(super) fn locate_symbol(text: &str, line_1based: usize, symbol: &str) -> Option<(u32, u32)> {
    if line_1based == 0 || symbol.is_empty() {
        return None;
    }
    let line = text.lines().nth(line_1based - 1)?;
    let byte_idx = line.find(symbol)?;
    let char0 = line[..byte_idx].encode_utf16().count() as u32;
    Some(((line_1based - 1) as u32, char0))
}

/// `file://` URI → ruta relativa al proyecto (`cwd`), con separadores `/`. Si la
/// URI cae fuera del proyecto, devuelve la ruta absoluta limpia (mejor mostrar
/// algo útil que nada). `None` si no es una `file://` URI.
fn uri_to_rel(cwd: &Path, uri: &str) -> Option<String> {
    let body = uri.strip_prefix("file://")?;
    // file:///C:/x → /C:/x en Windows: quita la barra inicial sobrante.
    let cleaned = if cfg!(windows) {
        body.strip_prefix('/').unwrap_or(body)
    } else {
        body
    };
    let decoded = cleaned.replace("%20", " ");
    let abs = std::path::PathBuf::from(decoded.replace('/', std::path::MAIN_SEPARATOR_STR));
    let rel = abs.strip_prefix(cwd).unwrap_or(&abs);
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Formatea las ubicaciones de referencias para el modelo (rutas relativas,
/// líneas/cols 1-based, ordenadas y sin duplicados).
pub(super) fn format_locations(cwd: &Path, rel_path: &str, symbol: &str, locations: &[Value]) -> String {
    if locations.is_empty() {
        return format!(
            "[find_references: el language server no reportó referencias a `{symbol}` \
             (puede seguir indexando, o el símbolo solo se usa en su declaración). \
             Si esperabas más, reintenta en unos segundos o usa search_project.]"
        );
    }
    let mut lines: Vec<String> = locations
        .iter()
        .map(|loc| {
            let uri = loc["uri"].as_str().unwrap_or("");
            let l = loc["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
            let c = loc["range"]["start"]["character"].as_u64().unwrap_or(0) + 1;
            let path = uri_to_rel(cwd, uri).unwrap_or_else(|| uri.to_string());
            format!("{path}:{l}:{c}")
        })
        .collect();
    lines.sort();
    lines.dedup();
    format!(
        "{} referencia(s) a `{symbol}` (declarado/usado desde `{rel_path}`):\n{}",
        lines.len(),
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::path_to_uri;
    use std::path::PathBuf;

    #[test]
    fn locate_symbol_resuelve_posicion_utf16() {
        let text = "fn main() {}\n    let run_turn = 1;\nfin\n";
        // Línea 2 (1-based), símbolo run_turn empieza tras 4 espacios + "let ".
        assert_eq!(locate_symbol(text, 2, "run_turn"), Some((1, 8)));
        // Línea inexistente o símbolo ausente → None.
        assert_eq!(locate_symbol(text, 99, "run_turn"), None);
        assert_eq!(locate_symbol(text, 1, "ausente"), None);
        assert_eq!(locate_symbol(text, 0, "main"), None);
        // Caracteres no-ASCII antes del símbolo cuentan en unidades UTF-16.
        let acc = "let café_x = 0;\n"; // 'é' = 1 unidad UTF-16
        assert_eq!(locate_symbol(acc, 1, "café_x"), Some((0, 4)));
    }

    #[test]
    fn format_locations_vacio_y_con_refs() {
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\proj" } else { "/proj" });
        assert!(format_locations(&cwd, "src/a.rs", "foo", &[]).contains("no reportó referencias"));

        let uri = path_to_uri(&cwd.join("src").join("b.rs"));
        let locs = vec![serde_json::json!({
            "uri": uri,
            "range": { "start": { "line": 41, "character": 7 } }
        })];
        let out = format_locations(&cwd, "src/a.rs", "foo", &locs);
        assert!(out.contains("1 referencia"));
        assert!(out.contains("src/b.rs:42:8"), "0-based→1-based y ruta relativa, vi: {out}");
    }

    #[test]
    fn position_to_byte_offsets_y_clamps() {
        let c = "fn main() {}\n  let x = 1;\nfin\n";
        // Inicio del archivo.
        assert_eq!(position_to_byte(c, 0, 0), Some(0));
        // Línea 1 (0-based), carácter 6 → 'x' tras "  let ": 13 ("fn main() {}\n") + 6 = 19.
        assert_eq!(position_to_byte(c, 1, 6), Some(19));
        // Carácter que rebasa la línea → clampa al fin del texto de esa línea.
        assert_eq!(position_to_byte(c, 1, 999), Some(13 + "  let x = 1;".len()));
        // Línea una más allá de la última → fin del archivo.
        assert_eq!(position_to_byte(c, 99, 0), None);
        // Carácter en unidades UTF-16 con no-ASCII antes ('é' = 1 unidad, 2 bytes).
        let u = "let café = 1;\n";
        // "let café": l(0)e(1)t(2) (3)c(4)a(5)f(6)é(7); char_utf16=7 → 'é' en byte 7.
        assert_eq!(position_to_byte(u, 0, 7), Some(7));
        // char 8 = justo después de 'é' (que ocupa 2 bytes) → byte 9.
        assert_eq!(position_to_byte(u, 0, 8), Some(9));
    }

    #[test]
    fn apply_text_edits_renombra_consistente() {
        let content = "let foo = 1;\nbar(foo, foo);\n";
        // Tres ocurrencias de `foo` → `baz`, en orden cualquiera (la fn ordena).
        let edits = vec![
            serde_json::json!({ "range": { "start": {"line":0,"character":4}, "end": {"line":0,"character":7} }, "newText": "baz" }),
            serde_json::json!({ "range": { "start": {"line":1,"character":4}, "end": {"line":1,"character":7} }, "newText": "baz" }),
            serde_json::json!({ "range": { "start": {"line":1,"character":9}, "end": {"line":1,"character":12} }, "newText": "baz" }),
        ];
        let out = apply_text_edits(content, &edits).unwrap();
        assert_eq!(out, "let baz = 1;\nbar(baz, baz);\n");
    }

    #[test]
    fn apply_workspace_edit_documentchanges_y_notas() {
        let cwd = std::env::temp_dir().join(format!("dpx-ws-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&cwd);
        std::fs::write(cwd.join("a.rs"), "let foo = 1;\n").unwrap();
        let uri = path_to_uri(&cwd.join("a.rs"));
        let edit = serde_json::json!({
            "documentChanges": [
                { "textDocument": { "uri": uri, "version": 1 },
                  "edits": [ { "range": { "start": {"line":0,"character":4}, "end": {"line":0,"character":7} }, "newText": "baz" } ] },
                { "kind": "rename", "oldUri": "x", "newUri": "y" }
            ]
        });
        let (writes, notes) = apply_workspace_edit(&cwd, &edit).unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "a.rs");
        assert_eq!(writes[0].content, "let baz = 1;\n");
        assert_eq!(notes.len(), 1, "la op de recurso `rename` debe generar una nota");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn uri_to_rel_relativiza_dentro_del_proyecto() {
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\proj" } else { "/proj" });
        let uri = path_to_uri(&cwd.join("src").join("main.rs"));
        assert_eq!(uri_to_rel(&cwd, &uri).as_deref(), Some("src/main.rs"));
    }
}
