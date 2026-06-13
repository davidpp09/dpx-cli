//! Definiciones de las herramientas nativas (function calling) de dpx.
//!
//! Es la versión estructurada de los bloques `dpx:*`: el modelo emite tool
//! calls con JSON validado por la API (se acabaron los bloques malformados) y
//! dpx los intercepta ANTES de ejecutar — las confirmaciones siguen siendo
//! nuestras. Los bloques de texto se mantienen como fallback para modelos
//! que no cooperen con tools.

use rig_core::completion::ToolDefinition;
use serde_json::{Value, json};

/// Una llamada a herramienta ya parseada y validada.
#[derive(Debug, PartialEq, Eq)]
pub enum DpxCall {
    Read { path: String },
    Search { pattern: String },
    Write { path: String, content: String },
    Edit { path: String, search: String, replace: String },
    Delete { path: String },
    Run { command: String },
    WebSearch { query: String },
    GitStatus,
    GitDiff { path: Option<String> },
    GitLog { n: Option<usize> },
    GitCommit { message: String },
    /// Tool de un servidor MCP externo (prefijo `mcp__<server>__<tool>`).
    McpTool { name: String, args: Value },
}

/// Las definiciones que se anuncian al modelo en cada petición.
/// Fusiona las 11 tools nativas con las tools MCP cacheadas (si las hay).
pub fn definitions() -> Vec<ToolDefinition> {
    let mut defs = native_definitions();
    for tool in crate::mcp::McpManager::cached_tools() {
        defs.push(ToolDefinition {
            name: tool.name,
            description: tool.description,
            parameters: tool.input_schema,
        });
    }
    defs
}

/// Solo las 11 definiciones nativas, sin MCP.
fn native_definitions() -> Vec<ToolDefinition> {
    fn def(name: &str, description: &str, props: Value, required: &[&str]) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            parameters: json!({
                "type": "object",
                "properties": props,
                "required": required,
            }),
        }
    }

    let path = |desc: &str| json!({ "type": "string", "description": desc });

    vec![
        def(
            "read_file",
            "Lee un archivo del proyecto y te devuelve su contenido. Úsala siempre que \
             necesites ver código existente: NUNCA le pidas al usuario que te pegue archivos.",
            json!({ "path": path("Ruta relativa al archivo, p.ej. src/main/java/App.java") }),
            &["path"],
        ),
        def(
            "search_project",
            "Busca un término en todos los archivos del proyecto y devuelve las coincidencias \
             con su ubicación.",
            json!({ "pattern": { "type": "string", "description": "Término o fragmento a buscar" } }),
            &["pattern"],
        ),
        def(
            "write_file",
            "Crea o SOBRESCRIBE un archivo completo. El usuario verá un diff y debe confirmar. \
             Para cambios puntuales en un archivo existente prefiere edit_file.",
            json!({
                "path": path("Ruta relativa del archivo a escribir"),
                "content": { "type": "string", "description": "Contenido COMPLETO del archivo" },
            }),
            &["path", "content"],
        ),
        def(
            "edit_file",
            "Edita un fragmento de un archivo: busca `search` LITERAL (copia exacta, con su \
             indentación) y lo reemplaza por `replace`. El usuario verá un diff y debe confirmar. \
             Si el texto no se encuentra, la edición falla sin tocar el archivo.",
            json!({
                "path": path("Ruta relativa del archivo a editar"),
                "search": { "type": "string", "description": "Texto exacto a buscar (primera aparición)" },
                "replace": { "type": "string", "description": "Texto que lo reemplaza" },
            }),
            &["path", "search", "replace"],
        ),
        def(
            "delete_file",
            "Borra un archivo del proyecto, con confirmación del usuario.",
            json!({ "path": path("Ruta relativa del archivo a borrar") }),
            &["path"],
        ),
        def(
            "run_command",
            "Ejecuta un comando de shell en la raíz del proyecto, con confirmación del usuario, \
             y te devuelve su salida. Debe terminar solo (hay timeout): nada de servidores ni \
             modo watch. Los comandos destructivos exigen confirmación reforzada y los que tocan \
             el sistema están prohibidos: propón siempre la alternativa más segura.",
            json!({ "command": { "type": "string", "description": "Comando a ejecutar, p.ej. mvn -q compile" } }),
            &["command"],
        ),
        def(
            "web_search",
            "Busca en Internet (DuckDuckGo) y devuelve los primeros resultados con título, \
             URL y fragmento. Úsala para consultar documentación actual, stackoverflow, \
             versiones recientes de librerías, errores conocidos o cualquier info que no \
             tengas en tu entrenamiento. Gratuita, sin API key.",
            json!({ "query": { "type": "string", "description": "Consulta de búsqueda, p.ej. 'rust axum 0.8 middleware' o 'java 21 virtual threads best practices'" } }),
            &["query"],
        ),
        def(
            "git_status",
            "Muestra el estado de git: archivos modificados, staged y untracked. Solo lectura, \
             sin confirmación.",
            json!({}),
            &[],
        ),
        def(
            "git_diff",
            "Muestra el diff del working tree (cambios sin commit). Opcionalmente, \
             de un archivo específico. Solo lectura, sin confirmación.",
            json!({ "path": { "type": "string", "description": "Archivo opcional: diff de un solo archivo" } }),
            &[],
        ),
        def(
            "git_log",
            "Muestra los últimos N commits (por defecto 10), una línea cada uno. Solo lectura, \
             sin confirmación.",
            json!({ "n": { "type": "integer", "description": "Cantidad de commits a mostrar (por defecto 10)" } }),
            &[],
        ),
        def(
            "git_commit",
            "Crea un commit con todos los cambios (git add -A + git commit). MUTA el repositorio: \
             requiere confirmación del usuario (o modo auto). Usa un mensaje descriptivo en \
             español.",
            json!({ "message": { "type": "string", "description": "Mensaje del commit" } }),
            &["message"],
        ),
    ]
}

/// Valida y convierte una llamada cruda (nombre + argumentos JSON) en un
/// [`DpxCall`]. El `Err` es un mensaje pensado para devolvérselo al modelo.
pub fn parse_call(name: &str, args: &Value) -> Result<DpxCall, String> {
    let arg = |key: &str| -> Result<String, String> {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("falta el argumento `{key}` (string) en la llamada a `{name}`"))
    };
    match name {
        "read_file" => Ok(DpxCall::Read { path: arg("path")? }),
        "search_project" => Ok(DpxCall::Search { pattern: arg("pattern")? }),
        "write_file" => Ok(DpxCall::Write { path: arg("path")?, content: arg("content")? }),
        "edit_file" => Ok(DpxCall::Edit {
            path: arg("path")?,
            search: arg("search")?,
            replace: arg("replace")?,
        }),
        "delete_file" => Ok(DpxCall::Delete { path: arg("path")? }),
        "run_command" => Ok(DpxCall::Run { command: arg("command")? }),
        "web_search" => Ok(DpxCall::WebSearch { query: arg("query")? }),
        "git_status" => Ok(DpxCall::GitStatus),
        "git_diff" => {
            let path = args.get("path").and_then(Value::as_str).map(str::to_string);
            Ok(DpxCall::GitDiff { path })
        }
        "git_log" => {
            let n = args.get("n").and_then(Value::as_u64).map(|v| v as usize);
            Ok(DpxCall::GitLog { n })
        }
        "git_commit" => Ok(DpxCall::GitCommit { message: arg("message")? }),
        other if other.starts_with("mcp__") => Ok(DpxCall::McpTool {
            name: other.to_string(),
            args: args.clone(),
        }),
        other => Err(format!(
            "herramienta desconocida: `{other}`. Las disponibles son: read_file, search_project, \
             write_file, edit_file, delete_file, run_command, web_search, git_status, git_diff, \
             git_log, git_commit."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definiciones_completas_y_con_schema() {
        let defs = definitions();
        assert!(defs.len() >= 11, "esperaba al menos 11 tools nativas");
        for d in &defs {
            assert!(!d.description.is_empty());
            assert_eq!(d.parameters["type"], "object");
            assert!(d.parameters["required"].is_array());
        }
    }

    #[test]
    fn parse_call_valida_argumentos() {
        let ok = parse_call("read_file", &json!({ "path": "src/main.rs" }));
        assert_eq!(ok, Ok(DpxCall::Read { path: "src/main.rs".into() }));

        let edit = parse_call(
            "edit_file",
            &json!({ "path": "a.rs", "search": "viejo", "replace": "nuevo" }),
        );
        assert!(matches!(edit, Ok(DpxCall::Edit { .. })));

        // Argumento faltante → error explicable al modelo, no panic.
        let err = parse_call("write_file", &json!({ "path": "a.rs" })).unwrap_err();
        assert!(err.contains("content"));

        let desconocida = parse_call("rm_rf", &json!({})).unwrap_err();
        assert!(desconocida.contains("desconocida"));
    }
}
