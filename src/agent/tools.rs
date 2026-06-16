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
    /// Lanza un subagente AISLADO de solo lectura para una tarea acotada; solo
    /// su conclusión vuelve al agente principal (ahorra contexto). `role` elige
    /// la especialidad (researcher por defecto); ver `agent::roles`.
    Spawn { task: String, role: Option<String> },
    /// Diagnósticos REALES de un archivo vía language server (errores/warnings).
    LspDiagnostics { path: String },
    /// Referencias REALES (ground truth del compilador) a un símbolo, vía LSP.
    FindReferences { path: String, line: usize, symbol: String },
    /// Renombra un símbolo en TODO el proyecto vía LSP (refactor exacto).
    RenameSymbol { path: String, line: usize, symbol: String, new_name: String },
    GitStatus,
    GitDiff { path: Option<String> },
    GitLog { n: Option<usize> },
    GitCommit { message: String },
    /// Tool de un servidor MCP externo (prefijo `mcp__<server>__<tool>`).
    McpTool { name: String, args: Value },
}

/// Las definiciones que se anuncian al modelo en cada petición.
/// Fusiona las 15 tools nativas con las tools MCP cacheadas (si las hay).
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

/// Solo las 15 definiciones nativas, sin MCP.
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
            &format!(
                "Lanza un SUBAGENTE AISLADO para una tarea acotada de lectura/análisis. Corre en \
                 el cerebro BARATO y en su PROPIO contexto: solo te devuelve su conclusión en \
                 texto, sin llenar TU contexto con archivos largos (ahorra dinero y foco). \
                 DELEGA de forma agresiva. Es de SOLO LECTURA: no escribe, edita, ejecuta ni \
                 commitea — para ACTUAR usa tus propias herramientas a partir de su conclusión. \
                 Elige el `role` adecuado: {roster}. Dale una tarea clara y autosuficiente \
                 (incluye las rutas que ya conozcas; no comparte tu conversación).",
                roster = crate::agent::roles::roster_blurb()
            ),
            json!({
                "task": { "type": "string", "description": "La tarea, específica y autosuficiente, p.ej. 'Localiza dónde se valida el token JWT y resume el flujo'" },
                "role": {
                    "type": "string",
                    "enum": crate::agent::roles::AgentRole::all().iter().map(|r| r.name()).collect::<Vec<_>>(),
                    "description": "Especialidad del subagente (opcional; researcher por defecto)"
                },
            }),
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
            "find_references",
            "Encuentra TODAS las referencias a un símbolo (función, variable, tipo, método) en el \
             proyecto usando el language server — ground truth del COMPILADOR, no texto. Mucho \
             más fiable que search_project para esto: no trae falsos positivos (otra cosa con el \
             mismo nombre) ni se pierde usos. ÚSALA antes de renombrar o borrar algo, para ver \
             qué se rompería. Indica el archivo donde está el símbolo, la línea (1-based, la que \
             ves al leer) y el NOMBRE del símbolo. Soporta .rs, .ts/.tsx, .js/.jsx, .py y .go; si \
             el language server no está instalado te lo dice (no falla la tarea).",
            json!({
                "path": path("Archivo donde aparece el símbolo, p.ej. src/cli/chat/mod.rs"),
                "line": { "type": "integer", "description": "Línea (1-based) donde está el símbolo en ese archivo" },
                "symbol": { "type": "string", "description": "Nombre exacto del símbolo, p.ej. run_turn" },
            }),
            &["path", "line", "symbol"],
        ),
        def(
            "rename_symbol",
            "Renombra un símbolo (función, variable, tipo, método) en TODO el proyecto usando el \
             language server: un refactor EXACTO calculado por el compilador, no un \
             find-and-replace. Actualiza la declaración y TODAS sus referencias de forma \
             consistente; el usuario verá un diff por archivo y confirma. ÚSALO en vez de editar \
             a mano archivo por archivo cuando cambies el nombre de algo usado en varios sitios. \
             Indica el archivo donde está el símbolo, la línea (1-based), su nombre ACTUAL y el \
             NUEVO. Soporta .rs, .ts/.tsx, .js/.jsx, .py y .go.",
            json!({
                "path": path("Archivo donde aparece el símbolo, p.ej. src/lsp.rs"),
                "line": { "type": "integer", "description": "Línea (1-based) donde está el símbolo" },
                "symbol": { "type": "string", "description": "Nombre ACTUAL del símbolo, p.ej. run_turn" },
                "new_name": { "type": "string", "description": "Nombre NUEVO, p.ej. ejecutar_turno" },
            }),
            &["path", "line", "symbol", "new_name"],
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

/// Extrae el argumento `line` (entero 1-based) con un error explicable si falta.
fn line_arg(args: &Value, name: &str) -> Result<usize, String> {
    args.get("line")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .ok_or_else(|| format!("falta el argumento `line` (entero) en la llamada a `{name}`"))
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
        "spawn_agent" => Ok(DpxCall::Spawn {
            task: arg("task")?,
            role: args.get("role").and_then(Value::as_str).map(str::to_string),
        }),
        "lsp_diagnostics" => Ok(DpxCall::LspDiagnostics { path: arg("path")? }),
        "find_references" => Ok(DpxCall::FindReferences {
            path: arg("path")?,
            line: line_arg(args, name)?,
            symbol: arg("symbol")?,
        }),
        "rename_symbol" => Ok(DpxCall::RenameSymbol {
            path: arg("path")?,
            line: line_arg(args, name)?,
            symbol: arg("symbol")?,
            new_name: arg("new_name")?,
        }),
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
             lsp_diagnostics, find_references, rename_symbol, git_status, git_diff, git_log, \
             git_commit."
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
        assert_eq!(
            spawn,
            Ok(DpxCall::Spawn { task: "investiga el flujo de auth".into(), role: None })
        );

        // Con rol explícito.
        let spawn_role = parse_call(
            "spawn_agent",
            &json!({ "task": "revisa fs/mod.rs", "role": "reviewer" }),
        );
        assert_eq!(
            spawn_role,
            Ok(DpxCall::Spawn { task: "revisa fs/mod.rs".into(), role: Some("reviewer".into()) })
        );

        // find_references: requiere path + line (entero) + symbol.
        let refs = parse_call(
            "find_references",
            &json!({ "path": "src/lib.rs", "line": 42, "symbol": "run_turn" }),
        );
        assert_eq!(
            refs,
            Ok(DpxCall::FindReferences {
                path: "src/lib.rs".into(),
                line: 42,
                symbol: "run_turn".into()
            })
        );
        // line faltante → error explicable, no panic.
        let sin_line = parse_call("find_references", &json!({ "path": "a.rs", "symbol": "x" })).unwrap_err();
        assert!(sin_line.contains("line"));

        // rename_symbol: path + line + symbol + new_name.
        let rename = parse_call(
            "rename_symbol",
            &json!({ "path": "src/lsp.rs", "line": 10, "symbol": "foo", "new_name": "bar" }),
        );
        assert_eq!(
            rename,
            Ok(DpxCall::RenameSymbol {
                path: "src/lsp.rs".into(),
                line: 10,
                symbol: "foo".into(),
                new_name: "bar".into()
            })
        );
        let sin_nuevo = parse_call("rename_symbol", &json!({ "path": "a.rs", "line": 1, "symbol": "x" })).unwrap_err();
        assert!(sin_nuevo.contains("new_name"));

        let desconocida = parse_call("rm_rf", &json!({})).unwrap_err();
        assert!(desconocida.contains("desconocida"));
    }
}
