//! Búsqueda web gratuita (sin API key) vía DuckDuckGo Lite.
//!
//! DuckDuckGo Lite no requiere API key: es un endpoint público no oficial.
//! Se usa para tareas que necesitan info fresca (docs, errores, librerías).
//!
//! # Límites
//! - Sin autenticación: ~10-15 requests/minuto desde una IP.
//! - Resultados limitados a titulares + URLs + fragmentos (sin ranking).
//! - No funciona desde China (bloqueado).

use anyhow::{Result, anyhow};

/// Máximo de resultados que devolvemos al modelo.
const MAX_RESULTS: usize = 5;

/// Busca con la API "instant answer" de DuckDuckGo (JSON, sin API key):
/// `https://api.duckduckgo.com/?q={query}&format=json&no_html=1&skip_disambig=1`
///
/// Devuelve `AbstractText`/`AbstractURL`/`AbstractSource` (el resultado
/// principal, si existe) y un array `RelatedTopics` (cada uno con `Text` y
/// `FirstURL`). Suficiente para dar contexto fresco al modelo sin parsear HTML.
pub async fn web_search(query: &str) -> Result<String> {
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencoding(query)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("dpx-cli/0.1 (mentor de programación)")
        .build()?;

    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        return Err(anyhow!("DuckDuckGo devolvió HTTP {}", resp.status()));
    }

    let raw: serde_json::Value = resp.json().await?;
    let mut out = String::new();
    let mut count = 0usize;

    // Resultado abstracto (el principal, si existe).
    if let Some(abstract_text) = raw["AbstractText"].as_str() {
        if !abstract_text.is_empty() {
            if let Some(source) = raw["AbstractSource"].as_str() {
                if let Some(url) = raw["AbstractURL"].as_str() {
                    count += 1;
                    out.push_str(&format!(
                        "  {count}. **{abstract_text}**\n     Fuente: [{source}]({url})\n\n"
                    ));
                }
            }
        }
    }

    // RelatedTopics: puede ser un array de objetos o de arrays (categorías).
    if let Some(topics) = raw["RelatedTopics"].as_array() {
        for topic in topics {
            if count >= MAX_RESULTS {
                break;
            }
            // Si es un array, es una categoría con subtopics; los aplanamos.
            if let Some(subtopics) = topic.as_array() {
                for sub in subtopics {
                    if count >= MAX_RESULTS {
                        break;
                    }
                    count = extract_topic(sub, &mut out, count)?;
                }
            } else {
                count = extract_topic(topic, &mut out, count)?;
            }
        }
    }

    if count == 0 {
        out.push_str("  (sin resultados)\n");
    }

    Ok(out)
}

/// Extrae un topic individual (objeto con `Text`, `FirstURL`) y lo añade a
/// la salida. Devuelve el nuevo contador.
fn extract_topic(topic: &serde_json::Value, out: &mut String, mut count: usize) -> Result<usize> {
    let text = topic["Text"].as_str().unwrap_or("");
    let url = topic["FirstURL"].as_str().unwrap_or("");
    if text.is_empty() {
        return Ok(count);
    }
    count += 1;
    // El texto viene con los resultados viejos de DDG: "titulo - descripción".
    // Intentamos separar.
    let (title, snippet) = if let Some(dash) = text.find(" - ") {
        let (t, s) = text.split_at(dash);
        (t.trim(), s[3..].trim())
    } else {
        (text, "")
    };
    out.push_str(&format!("  {count}. **{title}**\n"));
    if !snippet.is_empty() {
        out.push_str(&format!("     {snippet}\n"));
    }
    if !url.is_empty() {
        out.push_str(&format!("     [{url}]({url})\n"));
    }
    out.push('\n');
    Ok(count)
}

/// URL-encode manual (evitamos añadir `urlencoding` crate para una función).
fn urlencoding(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 2);
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencoding_basico() {
        assert_eq!(urlencoding("hola mundo"), "hola%20mundo");
        assert_eq!(urlencoding("rust+lang"), "rust%2Blang");
        assert_eq!(urlencoding("año"), "a%C3%B1o");
    }

    #[test]
    fn extract_topic_normal() {
        let json = serde_json::json!({
            "Text": "Rust (lenguaje de programación) - Wikipedia",
            "FirstURL": "https://es.wikipedia.org/wiki/Rust"
        });
        let mut out = String::new();
        let count = extract_topic(&json, &mut out, 0).unwrap();
        assert_eq!(count, 1);
        assert!(out.contains("Rust"));
        assert!(out.contains("Wikipedia"));
    }

    #[test]
    fn extract_topic_vacio() {
        let json = serde_json::json!({ "Text": "", "FirstURL": "" });
        let mut out = String::new();
        let count = extract_topic(&json, &mut out, 5).unwrap();
        assert_eq!(count, 5);
        assert!(out.is_empty());
    }

    /// Test de RED real: se ignora en `cargo test` normal (sin internet o con
    /// DDG caído rompería la suite). Correr a mano: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "requiere red"]
    async fn busqueda_real_funciona() {
        let results = web_search("rust programming language").await.unwrap();
        assert!(!results.is_empty(), "debe devolver algo");
        assert!(results.contains("Rust"), "debe mencionar Rust");
        println!("{results}");
    }

    #[tokio::test]
    #[ignore = "requiere red"]
    async fn busqueda_rara_no_crashea() {
        // DDG devuelve lo que encuentra (o nada): solo aseguramos que no falla.
        let _ = web_search("xyznonexistent999999").await.unwrap_or_default();
    }
}
