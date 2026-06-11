pub struct DiagnosticReport {
    pub hint: String,
    pub suggestions: Vec<String>,
}

/// Analiza la salida de un error y sugiere qué archivos investigar.
pub fn diagnose_failure(output: &str) -> Option<DiagnosticReport> {
    let mut suggestions = Vec::new();
    let mut hint = String::new();

    let out_lower = output.to_lowercase();

    // 1. Detección de duplicados (el error que tuvimos antes)
    if out_lower.contains("already defined") || out_lower.contains("duplicate class") {
        hint = "Parece que hay una clase o componente duplicado. Voy a sugerir buscarlo.".to_string();
        // Intentar extraer nombre simple (heurística básica)
        if let Some(cls) = output.split("class ").nth(1).and_then(|s| s.split_whitespace().next()) {
             suggestions.push(format!("dpx:search pattern={}", cls));
        } else {
             suggestions.push("dpx:search pattern=NOMBRE_DE_LA_CLASE".to_string());
        }
    }

    // 2. Error de base de datos / Flyway / Liquibase
    if out_lower.contains("schemamanagementexception") || out_lower.contains("table not found") {
        hint = "Hay un problema con el esquema de la base de datos o las migraciones no se han aplicado.".to_string();
        suggestions.push("dpx:read path=src/main/resources/application.yml".to_string());
        suggestions.push("dpx:run command=dir src\\main\\resources\\db\\migration".to_string());
    }

    // 3. ClassNotFound o NoClassDefFound
    if out_lower.contains("classnotfoundexception") || out_lower.contains("noclassdeffounderror") {
        hint = "Falta una dependencia o clase en el classpath.".to_string();
        suggestions.push("dpx:read path=pom.xml".to_string());
    }

    if suggestions.is_empty() {
        None
    } else {
        Some(DiagnosticReport { hint, suggestions })
    }
}
