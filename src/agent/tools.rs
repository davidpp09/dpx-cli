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
    Read { path: String, offset: Option<usize>, limit: Option<usize> },
    Search { pattern: String },
    Write { path: String, content: String },
    Edit { path: String, search: String, replace: String },
    Delete { path: String },
    Run { command: String },
    WebSearch { query: String },
    /// Lanza un subagente de investigación aislado (solo lectura) para una tarea
    /// acotada; solo su conclusión vuelve al agente principal (ahorra contexto).
    Spawn { task: String },
    /// Diagnósticos REALES de un archivo vía language server (errores/warnings).
    LspDiagnostics { path: String },
    GitStatus,
    GitDiff { path: Option<String> },
    GitLog { n: Option<usize> },
    GitCommit { message: String },
    /// Tool de un servidor MCP externo (prefijo `mcp__<server>__<tool>`).
    McpTool { name: String, args: Value },
}

/// Las definiciones que se anuncian al modelo en cada petición.
/// Fusiona las 13 tools nativas con las tools MCP cacheadas (si las hay).
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

/// Solo las 13 definiciones nativas, sin MCP.
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
             necesites ver código existente: NUNCA le pidas al usuario que te pegue archivos NI \
             escribas scripts (python/tail/etc.) para leer un archivo. Para archivos largos lee \
             un RANGO con `offset` (línea inicial, 1-based) y `limit` (nº de líneas): si la \
             salida dice cuántas líneas faltan, vuelve a llamar con el `offset` indicado para \
             ver el resto (p.ej. el final del archivo).",
            json!({
                "path": path("Ruta relativa al archivo, p.ej. src/main/java/App.java"),
                "offset": { "type": "integer", "description": "Opcional: línea inicial (1-based) para leer un rango de un archivo grande" },
                "limit": { "type": "integer", "description": "Opcional: máximo de líneas a leer desde offset" },
            }),
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
            "spawn_agent",
            "Lanza un SUBAGENTE de investigación AISLADO para una tarea de lectura acotada: \
             explorar el código, localizar dónde se hace algo, recopilar contexto de varios \
             archivos o investigar en la web. El subagente tiene su PROPIO contexto y solo te \
             devuelve su conclusión — úsalo para no llenar TU contexto con archivos largos \
             cuando solo necesitas el resumen. Es de SOLO LECTURA: no puede escribir, editar, \
             ejecutar ni commitear (para eso usa tus propias herramientas). Dale una tarea \
             clara y autosuficiente (no comparte tu conversación).",
            json!({ "task": { "type": "string", "description": "La tarea de investigación, específica y autosuficiente, p.ej. 'Localiza dónde se valida el token JWT y resume el flujo'" } }),
            &["task"],
        ),
        def(
            "lsp_diagnostics",
            "Devuelve los diagnósticos REALES (errores y warnings, con línea y columna) de un \
             archivo según su language server (rust-analyzer, typescript-language-server, \
             pyright, gopls). Es grounding de calidad de compilador SIN compilar el proyecto \
             entero: úsala para verificar puntualmente un archivo que editaste o para ubicar un \
             error con precisión. Si el language server no está instalado, te lo dice (no falla \
             la tarea). Soporta .rs, .ts/.tsx, .js/.jsx, .py y .go.",
            json!({ "path": path("Ruta relativa al archivo a diagnosticar, p.ej. src/main.rs") }),
            &["path"],
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
        "read_file" => Ok(DpxCall::Read {
            path: arg("path")?,
            offset: args.get("offset").and_then(Value::as_u64).map(|v| v as usize),
            limit: args.get("limit").and_then(Value::as_u64).map(|v| v as usize),
        }),
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
        "spawn_agent" => Ok(DpxCall::Spawn { task: arg("task")? }),
        "lsp_diagnostics" => Ok(DpxCall::LspDiagnostics { path: arg("path")? }),
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
             write_file, edit_file, delete_file, run_command, web_search, spawn_agent, \
             lsp_diagnostics, git_status, git_diff, git_log, git_commit."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definiciones_completas_y_con_schema() {
        let defs = definitions();
        assert!(defs.len() >= 13, "esperaba al menos 13 tools nativas");
        for d in &defs {
            assert!(!d.description.is_empty());
            assert_eq!(d.parameters["type"], "object");
            assert!(d.parameters["required"].is_array());
        }
    }

    #[test]
    fn parse_call_valida_argumentos() {
        let ok = parse_call("read_file", &json!({ "path": "src/main.rs" }));
        assert_eq!(ok, Ok(DpxCall::Read { path: "src/main.rs".into(), offset: None, limit: None }));

        // Con rango (offset/limit) para archivos grandes.
        let ranged = parse_call("read_file", &json!({ "path": "big.rs", "offset": 2501, "limit": 500 }));
        assert_eq!(
            ranged,
            Ok(DpxCall::Read { path: "big.rs".into(), offset: Some(2501), limit: Some(500) })
        );

        let edit = parse_call(
            "edit_file",
            &json!({ "path": "a.rs", "search": "viejo", "replace": "nuevo" }),
        );
        assert!(matches!(edit, Ok(DpxCall::Edit { .. })));

        // Argumento faltante → error explicable al modelo, no panic.
        let err = parse_call("write_file", &json!({ "path": "a.rs" })).unwrap_err();
        assert!(err.contains("content"));

        let spawn = parse_call("spawn_agent", &json!({ "task": "investiga el flujo de auth" }));
        assert_eq!(spawn, Ok(DpxCall::Spawn { task: "investiga el flujo de auth".into() }));

        let desconocida = parse_call("rm_rf", &json!({})).unwrap_err();
        assert!(desconocida.contains("desconocida"));
    }
}
