//! Ediciones quirúrgicas (`edit_file`): aplicación de un bloque SEARCH/REPLACE
//! sobre el contenido — exacta, tolerante a CRLF y fuzzy por indentación, con
//! pista útil si no encuentra. Extraído de `fs`.

use anyhow::{Result, anyhow};

/// Una edición quirúrgica propuesta: reemplaza `search` (texto literal del
/// archivo) por `replace`, sin reescribir el archivo entero.
pub struct FileEdit {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Aplica una edición sobre el contenido actual: busca `search` de forma LITERAL
/// (sin regex) y reemplaza su primera aparición.
///
/// Tolera diferencias de final de línea (CRLF vs LF), la causa nº 1 de fallos de
/// `edit_file` en Windows: el LLM emite el SEARCH con `\n` pero el archivo en
/// disco tiene `\r\n` (o una mezcla de ambos, p.ej. tras `git checkout` con
/// `core.autocrlf`). El reemplazo PRESERVA los finales de línea originales del
/// archivo, porque solo se sustituye el tramo coincidente.
///
/// Estrategia en capas, de la más estricta a la más tolerante (la primera que
/// acierta gana, así nunca degradamos un match exacto):
/// 1. **Exacto** — `str::find` literal; conserva el comportamiento previo.
/// 2. **Finales de línea** — normaliza CRLF↔LF en ambos lados y mapea el offset
///    de vuelta al original (cubre finales mixtos).
/// 3. **Indentación/espacios (fuzzy por línea)** — localiza el bloque comparando
///    líneas por su contenido SIN espacios de borde, así un SEARCH con la
///    indentación mal puesta (la causa nº 1 de fallos que quedaba) igual ubica
///    el tramo. Solo aplica si hay UNA coincidencia: si es ambigua, prefiere
///    fallar con pista a editar el lugar equivocado.
///
/// El reemplazo PRESERVA lo que hay alrededor: solo se sustituye el tramo
/// coincidente. Error claro con pista verbatim si ninguna capa acierta.
pub fn apply_edit(current: &str, edit: &FileEdit) -> Result<String> {
    // 1. Intento exacto.
    if let Some(idx) = current.find(&edit.search) {
        return Ok(splice(current, idx, idx + edit.search.len(), &edit.replace));
    }

    // 2. Tolerante a finales de línea: normaliza CRLF→LF en ambos lados y busca
    //    en el espacio normalizado; el `map` traduce el offset normalizado al
    //    byte original para cortar exactamente el tramo coincidente.
    let needle_norm = edit.search.replace("\r\n", "\n");
    let (current_norm, map) = normalize_lf_with_map(current);
    if let Some(n_idx) = current_norm.find(&needle_norm) {
        let start = map[n_idx];
        let end = map[n_idx + needle_norm.len()];
        return Ok(splice(current, start, end, &edit.replace));
    }

    // 3. Tolerante a indentación/espacios: match línea-a-línea por contenido
    //    recortado, solo si es inequívoco.
    if let Some((start, end)) = fuzzy_line_span(current, &edit.search) {
        return Ok(splice(current, start, end, &edit.replace));
    }

    Err(anyhow!(
        "no encontré el bloque SEARCH en `{}`: el texto no coincide con el archivo actual.{}",
        edit.path,
        search_hint(current, &edit.search)
    ))
}

/// Divide `s` en líneas devolviendo `(offset_de_byte_del_inicio, contenido)`,
/// donde `contenido` excluye el terminador (`\n` o `\r\n`). Sirve para mapear
/// una coincidencia por líneas de vuelta a offsets de byte exactos.
fn line_offsets(s: &str) -> Vec<(usize, &str)> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let start = i;
        let mut j = i;
        while j < n && bytes[j] != b'\n' {
            j += 1;
        }
        // Excluir el `\r` de un `\r\n` del contenido.
        let mut content_end = j;
        if content_end > start && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        out.push((start, &s[start..content_end]));
        if j >= n {
            break;
        }
        i = j + 1;
    }
    out
}

/// Localiza el bloque `search` en `current` comparando líneas por su contenido
/// recortado (ignora indentación y espacios de borde). Devuelve el rango de
/// bytes `[inicio, fin]` del tramo en `current` (de la 1ª línea a la última, sin
/// el salto final) SOLO si hay exactamente UNA coincidencia; `None` si hay cero
/// o varias (ambiguo → mejor no tocar). Requiere al menos una línea con
/// contenido real, para no "encontrar" un bloque de puras líneas en blanco.
fn fuzzy_line_span(current: &str, search: &str) -> Option<(usize, usize)> {
    let needle: Vec<&str> = search.lines().map(str::trim).collect();
    if needle.is_empty() || needle.iter().all(|l| l.is_empty()) {
        return None;
    }
    let lines = line_offsets(current);
    let n = needle.len();
    if lines.len() < n {
        return None;
    }
    let mut found: Option<usize> = None;
    for i in 0..=(lines.len() - n) {
        let matches = (0..n).all(|k| lines[i + k].1.trim() == needle[k]);
        if matches {
            if found.is_some() {
                return None; // ambiguo: ≥2 coincidencias
            }
            found = Some(i);
        }
    }
    let i = found?;
    let last = i + n - 1;
    let start = lines[i].0;
    let end = lines[last].0 + lines[last].1.len();
    Some((start, end))
}

/// Reemplaza `original[start..end]` por `replace`, devolviendo la cadena nueva.
fn splice(original: &str, start: usize, end: usize, replace: &str) -> String {
    let mut out = String::with_capacity(original.len() + replace.len());
    out.push_str(&original[..start]);
    out.push_str(replace);
    out.push_str(&original[end..]);
    out
}

/// Normaliza `\r\n` → `\n` y devuelve `(normalizado, map)`, donde `map[i]` es el
/// índice de byte en `original` del que proviene el byte `i` del normalizado.
/// `map` tiene `normalizado.len() + 1` entradas y `map[normalizado.len()]` apunta
/// a `original.len()`, de modo que cualquier rango `[a, b]` del normalizado se
/// traduce a `[map[a], map[b]]` en el original. Solo toca ASCII (`\r`/`\n`), así
/// que preserva la validez UTF-8 y los límites de carácter.
fn normalize_lf_with_map(original: &str) -> (String, Vec<usize>) {
    let bytes = original.as_bytes();
    let mut norm: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut map: Vec<usize> = Vec::with_capacity(bytes.len() + 1);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            // El '\n' del normalizado nace en el '\r' y consume 2 bytes.
            map.push(i);
            norm.push(b'\n');
            i += 2;
        } else {
            map.push(i);
            norm.push(bytes[i]);
            i += 1;
        }
    }
    map.push(bytes.len());
    // `norm` solo difiere de `original` en ASCII \r\n → \n: sigue siendo UTF-8
    // válido. El fallback a `original` jamás debería dispararse.
    let norm = String::from_utf8(norm).unwrap_or_else(|_| original.to_string());
    (norm, map)
}

/// Pista para un SEARCH fallido, pensada para que el MODELO se autocorrija en
/// vez de reintentar a ciegas: localiza en el archivo la zona más parecida
/// (anclando por la primera línea con contenido del search, comparada
/// ignorando espacios) y la devuelve VERBATIM para copiarla tal cual. La
/// causa nº 1 de estos fallos son diferencias invisibles: indentación,
/// escapes `\"` y los `\` de continuación en string literals.
fn search_hint(current: &str, search: &str) -> String {
    fn normalize(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    let Some(anchor) = search.lines().find(|l| !l.trim().is_empty()) else {
        return String::new();
    };
    let anchor_norm = normalize(anchor);
    let lines: Vec<&str> = current.lines().collect();
    let hit = lines.iter().position(|l| {
        let ln = normalize(l);
        ln == anchor_norm
            || (anchor_norm.chars().count() > 12
                && (ln.contains(&anchor_norm) || anchor_norm.contains(&ln) && !ln.is_empty()))
    });
    match hit {
        Some(i) => {
            let span = search.lines().count().clamp(3, 10);
            let end = (i + span).min(lines.len());
            let excerpt = lines[i..end].join("\n");
            format!(
                " La zona más parecida del archivo es esta — tu SEARCH difiere de ella en algo \
                 (espacios, `\\` de continuación o escapes). Copia este texto EXACTAMENTE:\n\
                 ---\n{excerpt}\n---"
            )
        }
        None => " Ni siquiera la primera línea de tu SEARCH aparece en el archivo (¿archivo o \
                 sección equivocada?). Relee el archivo fresco antes de reintentar."
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_edit_reemplaza_primera_aparicion() {
        let edit = FileEdit {
            path: "x".into(),
            search: "fn a() {}".into(),
            replace: "fn b(n: u8) {}".into(),
        };
        let out = apply_edit("antes\nfn a() {}\ndespués\n", &edit).unwrap();
        assert_eq!(out, "antes\nfn b(n: u8) {}\ndespués\n");
    }

    #[test]
    fn apply_edit_falla_si_no_encuentra() {
        let edit = FileEdit { path: "x".into(), search: "no está".into(), replace: "y".into() };
        assert!(apply_edit("contenido real", &edit).is_err());
    }

    #[test]
    fn apply_edit_tolera_crlf_en_archivo_con_search_lf() {
        // Caso real en Windows: el archivo tiene \r\n, el SEARCH del LLM usa \n.
        let current = "línea uno\r\nlínea dos\r\nlínea tres\r\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "línea uno\nlínea dos".into(),  // solo LF
            replace: "reemplazada".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        // El contenido se reemplaza respetando los \r\n originales.
        assert_eq!(out, "reemplazada\r\nlínea tres\r\n");
    }

    #[test]
    fn apply_edit_tolera_crlf_en_search_con_archivo_lf() {
        // Caso inverso (raro pero posible): archivo LF, SEARCH con CRLF.
        let current = "fn foo() {\n    bar();\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "fn foo() {\r\n    bar();".into(),
            replace: "fn baz() {".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "fn baz() {\n}\n");
    }

    #[test]
    fn apply_edit_exacto_sigue_funcionando() {
        // Sin diferencias CRLF, el comportamiento no cambia.
        let edit = FileEdit {
            path: "x".into(),
            search: "línea uno\nlínea dos".into(),
            replace: "reemplazada".into(),
        };
        let out = apply_edit("línea uno\nlínea dos\nlínea tres\n", &edit).unwrap();
        assert_eq!(out, "reemplazada\nlínea tres\n");
    }

    #[test]
    fn apply_edit_solo_lf_con_crlf_en_medio_del_contenido() {
        // El archivo mezcla CRLF y LF (pasa en la práctica: git checkout en
        // Windows con core.autocrlf=true puede dejar mezclas).
        let current = "cabecera\ncuerpo\r\npie\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "cuerpo".into(), // sin newlines, exact match basta
            replace: "nuevo".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "cabecera\nnuevo\r\npie\n");
    }

    #[test]
    fn apply_edit_tolera_finales_mixtos_multilinea() {
        // El bloque a editar ABARCA líneas con finales distintos: la primera
        // termina en \r\n y la segunda en \n. El SEARCH del LLM viene todo en
        // \n. El heurístico simple de \n↔\r\n NO resolvía esto (normalizaba a
        // todo-CRLF y no encontraba); el mapeo normalizado sí.
        let current = "uno\r\ndos\ntres\r\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "uno\ndos\ntres".into(), // todo LF, cruza CRLF y LF reales
            replace: "fusionado".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        // Reemplaza el tramo coincidente y preserva el \r\n final del archivo.
        assert_eq!(out, "fusionado\r\n");
    }

    #[test]
    fn apply_edit_finales_mixtos_preserva_cola_intacta() {
        // Verifica que el mapeo de offsets corta exactamente: el texto antes y
        // después del match queda byte-a-byte intacto, con sus finales propios.
        let current = "head\r\nfn f() {\n    body();\r\n}\ntail\r\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "fn f() {\n    body();\n}".into(), // LF, pero el archivo mezcla
            replace: "fn f() { done(); }".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "head\r\nfn f() { done(); }\ntail\r\n");
    }

    #[test]
    fn apply_edit_fuzzy_tolera_indentacion_mal_puesta() {
        // El modelo emitió el SEARCH SIN indentación; el archivo la tiene.
        // Ni el exacto ni el de CRLF lo encuentran; el fuzzy por línea sí.
        let current = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "let x = 1;\nprintln!(\"{}\", x);".into(), // sin los 4 espacios
            replace: "    let y = 2;\n    dbg!(y);".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "fn main() {\n    let y = 2;\n    dbg!(y);\n}\n");
    }

    #[test]
    fn apply_edit_fuzzy_una_sola_linea_con_espacios_de_sobra() {
        // El SEARCH trae espacios al final que el archivo no tiene → no es
        // substring literal (exacto/CRLF fallan), pero el fuzzy lo ubica. Como
        // el fuzzy reemplaza la LÍNEA completa (con su indentación), el REPLACE
        // debe traer la indentación deseada.
        let current = "a = 0\n    value = 1\nb = 2\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "value = 1   ".into(), // espacios sobrantes al final
            replace: "    value = 99".into(),
        };
        assert!(current.find(&edit.search).is_none(), "no debe ser match exacto");
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "a = 0\n    value = 99\nb = 2\n");
    }

    #[test]
    fn apply_edit_fuzzy_ambiguo_no_toca_y_da_error() {
        // El MISMO bloque (salvo indentación) aparece dos veces. El SEARCH sin
        // indentar no es substring literal (hay espacios entre las líneas en el
        // archivo), así que exacto/CRLF fallan y se llega al fuzzy; al haber dos
        // coincidencias, se abstiene y devuelve error en vez de editar a ciegas.
        let current =
            "fn a() {\n    do_x();\n    do_y();\n}\nfn b() {\n        do_x();\n        do_y();\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "do_x();\ndo_y();".into(),
            replace: "done();".into(),
        };
        assert!(fuzzy_line_span(current, &edit.search).is_none(), "debe ser ambiguo");
        assert!(apply_edit(current, &edit).is_err());
    }

    #[test]
    fn apply_edit_fuzzy_no_inventa_match_con_lineas_en_blanco() {
        // Un SEARCH de puras líneas en blanco no debe "encontrar" cualquier hueco.
        let current = "a\n\n\nb\n";
        assert!(fuzzy_line_span(current, "   \n  ").is_none());
        assert!(fuzzy_line_span(current, "").is_none());
    }

    #[test]
    fn search_fallido_da_pista_con_el_texto_real() {
        // La primera línea del search existe (con otra indentación), pero la
        // segunda no coincide: la pista debe traer el texto REAL del archivo
        // desde esa ancla, para que el modelo lo copie.
        let current =
            "fn x() {\n        let valor = calcular_total(precios); // exacto\n        valor + 1\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "let valor = calcular_total(precios); // exacto\n    return valor;".into(),
            replace: "y".into(),
        };
        let err = apply_edit(current, &edit).unwrap_err().to_string();
        assert!(err.contains("Copia este texto EXACTAMENTE"));
        assert!(err.contains("        let valor = calcular_total(precios); // exacto"));
    }

    #[test]
    fn search_sin_relacion_pide_releer() {
        let edit = FileEdit {
            path: "x".into(),
            search: "esta línea no existe en ninguna forma".into(),
            replace: "y".into(),
        };
        let err = apply_edit("contenido\ncompletamente distinto\n", &edit).unwrap_err().to_string();
        assert!(err.contains("Relee el archivo fresco"));
    }
}
