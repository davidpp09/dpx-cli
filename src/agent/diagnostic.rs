/// Diagnóstico automático de fallos de compilación/ejecución, multi-lenguaje.
///
/// Los detectores identfican el lenguaje por marcadores únicos del compilador
/// (rustc, tsc, Python, javac) y extraen el archivo + línea del error principal,
/// para ahorrar al modelo rondas de "¿y qué archivo miro?".
///
/// Si ningún detector específico casa, el genérico busca paths en la salida.

pub struct DiagnosticReport {
    pub hint: String,
    pub suggestions: Vec<String>,
}

/// Sugerencia de lectura a partir de una ubicación `path:línea`. Lee el
/// ARCHIVO (path SIN la línea — `read_file`/`dpx:read` recibe una ruta, y
/// `src/x.rs:15` no es una ruta válida) y menciona la línea como foco aparte.
fn read_hint(file_line: &str) -> String {
    match file_line.rsplit_once(':') {
        Some((path, line)) if !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()) => {
            format!("dpx:read path={path}  ← el error apunta a la línea {line}")
        }
        _ => format!("dpx:read path={file_line}"),
    }
}

/// Dispatch principal: prueba cada detector en orden hasta que uno casa.
pub fn diagnose_failure(output: &str) -> Option<DiagnosticReport> {
    diagnose_rust(output)
        .or_else(|| diagnose_typescript(output))
        .or_else(|| diagnose_java(output))
        .or_else(|| diagnose_python(output))
        .or_else(|| diagnose_generic(output))
}

// ═══════════════════════════════════════════════════════════════════════════
// Rust: compilador rustc/cargo
// ═══════════════════════════════════════════════════════════════════════════

fn diagnose_rust(output: &str) -> Option<DiagnosticReport> {
    // El error de rustc tiene un marcador único: "error[E" con código numérico.
    if !output.contains("error[E") {
        return None;
    }

    let file_line = extract_rust_location(output);
    let out_lower = output.to_lowercase();

    let (hint, suggestions) = if output.contains("E0597") {
        (
            "El borrow no vive lo suficiente (E0597). Típicamente una referencia \
             sobrevive al dato que la originó.".to_string(),
            vec![
                read_hint(&file_line),
                "dpx:search pattern=lifetime".to_string(),
                "Pista: probá mover la variable fuera del bloque, o usá un String/owned \
                 en lugar de &str en el struct".to_string(),
            ],
        )
    } else if output.contains("E0502") || output.contains("E0499") || output.contains("E0503") {
        (
            "Conflicto de borrow (E0502/E0499/E0503): tomás prestado mutable e inmutable \
             a la vez, o dos mutables simultáneos.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: acortá la vida de los borrows (bloques extra), usá `clone()`, \
                 o replanteá con índices / interior mutability.".to_string(),
            ],
        )
    } else if output.contains("E0382") {
        (
            "Use of moved value (E0382): estás usando un valor después de moverlo.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: cloná antes del move, o usá referencias (&) en lugar de pasar ownership.".to_string(),
            ],
        )
    } else if output.contains("E0277") {
        (
            "Falta un trait bound (E0277): un tipo no implementa el trait que se espera.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿derive? ¿importás el trait? Revisá los bounds del generic o \
                 añadí `#[derive(...)]` o `impl Trait for Type`.".to_string(),
            ],
        )
    } else if output.contains("E0282") || output.contains("E0283") {
        (
            "El compilador no pudo inferir un tipo (E0282/E0283): necesita anotación \
             explícita o el tipo es ambiguo.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: anotá el tipo explícitamente (`let x: Type = ...`) o usá \
                 el turbofish (`::<Type>`) para guiar la inferencia.".to_string(),
            ],
        )
    } else if output.contains("E0308") {
        (
            "Type mismatch (E0308): esperabas un tipo pero encontraste otro.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: revisá el mensaje exacto del compilador (expected vs found); \
                 puede que estés pasando &String donde se espera &str, o un Result \
                 donde esperabas el valor desempaquetado.".to_string(),
            ],
        )
    } else if output.contains("E0432") || output.contains("E0433") {
        (
            "Import no resuelto (E0432/E0433): módulo, crate o feature no encontrado.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿está el crate en Cargo.toml con la feature correcta? ¿el \
                 módulo se declara con `mod nombre;` en el padre?".to_string(),
            ],
        )
    } else if output.contains("E0425") || output.contains("E0423") {
        (
            "Símbolo no encontrado (E0425/E0423): función, variable o struct que \
             no existe en este scope.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿faltó un `use`? ¿typo en el nombre? ¿está en otro módulo?".to_string(),
            ],
        )
    } else if output.contains("E0603") {
        (
            "Acceso a miembro privado (E0603): un módulo o campo no es público.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: hacé el campo/función `pub`, o usá el constructor público \
                 del tipo.".to_string(),
            ],
        )
    } else if out_lower.contains("unresolved") || out_lower.contains("could not compile") {
        (
            "Error de compilación de Rust (revisá el mensaje exacto arriba).".to_string(),
            vec![
                read_hint(&file_line),
                format!("dpx:run command=cargo check 2>&1 | head -40"),
            ],
        )
    } else {
        return None;
    };

    Some(DiagnosticReport { hint, suggestions })
}

/// Extrae "src/file.rs:42" de una línea `--> src/file.rs:42:10`.
fn extract_rust_location(output: &str) -> String {
    output
        .lines()
        .find_map(|line| {
            let t = line.trim();
            t.strip_prefix("--> ").map(|rest| {
                // Quitar la columna: "src/file.rs:42:10" → "src/file.rs:42"
                if let Some(idx) = rest.rfind(':') {
                    if rest[..idx].rfind(':').is_some() {
                        return rest[..idx].to_string();
                    }
                }
                rest.to_string()
            })
        })
        .unwrap_or_else(|| "src/main.rs".to_string())
}

// ═══════════════════════════════════════════════════════════════════════════
// TypeScript / JavaScript: compilador tsc
// ═══════════════════════════════════════════════════════════════════════════

fn diagnose_typescript(output: &str) -> Option<DiagnosticReport> {
    // tsc emite `error TS` con código de 4 dígitos.
    if !output.contains("error TS") {
        return None;
    }

    let file_line = extract_ts_location(output);

    let (hint, suggestions) = if output.contains("TS2345") {
        (
            "TypeScript: argumento de tipo incorrecto (TS2345). Pasás un tipo donde \
             se espera otro.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: revisá la firma de la función y el tipo del argumento; \
                 quizá falta una prop obligatoria o el tipo es más estrecho.".to_string(),
            ],
        )
    } else if output.contains("TS2322") {
        (
            "TypeScript: tipo asignado incompatible (TS2322).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: el tipo de la variable no admite el valor que le das; \
                 puede que falte un campo o que el tipo sea `string` y le pases `number`.".to_string(),
            ],
        )
    } else if output.contains("TS2339") {
        (
            "TypeScript: propiedad inexistente (TS2339). Estás accediendo a algo \
             que el tipo no declara.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿faltó declarar la propiedad en la interfaz/tipo? ¿usaste \
                 `any` antes y ahora tipaste correctamente?".to_string(),
            ],
        )
    } else if output.contains("TS2304") || output.contains("TS2307") {
        (
            "TypeScript: nombre o módulo no encontrado (TS2304/TS2307).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿falta un import? ¿el paquete está en package.json y en \
                 node_modules? ¿typo en el nombre?".to_string(),
            ],
        )
    } else if output.contains("TS2554") {
        (
            "TypeScript: número incorrecto de argumentos (TS2554).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: la función espera más (o menos) parámetros de los que le \
                 estás pasando. Revisá la firma.".to_string(),
            ],
        )
    } else if output.contains("TS7006") {
        (
            "TypeScript: parámetro con tipo implícito `any` (TS7006).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: anotá el tipo del parámetro explícitamente, o si es un \
                 callback, inferilo de la firma de la función.".to_string(),
            ],
        )
    } else if output.contains("TS2769") {
        (
            "TypeScript: no hay overload que case (TS2769).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: revisá las signaturas de overload y el tipo de cada argumento \
                 en la llamada.".to_string(),
            ],
        )
    } else if output.contains("TS2741") || output.contains("TS2739") {
        (
            "TypeScript: faltan propiedades requeridas (TS2741/TS2739).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: el objeto que estás creando no incluye todos los campos \
                 obligatorios de la interfaz/tipo.".to_string(),
            ],
        )
    } else {
        // Genérico TS: al menos extraemos la ubicación.
        (
            "Error de TypeScript (revisá el código exacto arriba para más detalle).".to_string(),
            vec![
                read_hint(&file_line),
                "dpx:read path=tsconfig.json".to_string(),
            ],
        )
    };

    Some(DiagnosticReport { hint, suggestions })
}

/// Extrae "src/file.ts:42" de `src/file.ts(42,10): error TS2345:`.
fn extract_ts_location(output: &str) -> String {
    for line in output.lines() {
        if let Some(rest) = line.trim().split(": error TS").next() {
            // "src/file.ts(42,10)" → "src/file.ts:42"
            if let Some((path, pos)) = rest.split_once('(') {
                let pos = pos.trim_end_matches(')');
                if let Some((row, _col)) = pos.split_once(',') {
                    return format!("{path}:{row}");
                }
                return format!("{path}:{pos}");
            }
            return rest.to_string();
        }
    }
    "src/index.ts".to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// Python
// ═══════════════════════════════════════════════════════════════════════════

fn diagnose_python(output: &str) -> Option<DiagnosticReport> {
    // Traceback de runtime o línea `File "x.py", line N` de error de compilación.
    let has_python_marker = output.contains("Traceback (most recent call last):")
        || (output.contains("File \"") && output.contains("\", line "));
    if !has_python_marker {
        return None;
    }

    let file_line = extract_python_location(output);

    let (hint, suggestions) = if output.contains("ModuleNotFoundError")
        || output.contains("ImportError")
    {
        (
            "Python: no se encontró un módulo (ModuleNotFoundError / ImportError).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿está instalado el paquete? (`uv add ...` o `pip install ...`). \
                 ¿el módulo local existe en ese path? ¿falta un __init__.py?".to_string(),
            ],
        )
    } else if output.contains("AttributeError") {
        (
            "Python: intentaste acceder a un atributo que no existe (AttributeError).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿typo en el nombre del método/atributo? ¿el objeto es del \
                 tipo que esperabas (quizá es None)? ¿faltó importar algo?".to_string(),
            ],
        )
    } else if output.contains("TypeError") {
        (
            "Python: operación con tipo incompatible (TypeError).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: fijate en el mensaje exacto: suele decir qué tipo esperaba \
                 y qué recibió. ¿Le pasaste un str a algo que esperaba int? ¿None \
                 en vez de un objeto?".to_string(),
            ],
        )
    } else if output.contains("NameError") {
        (
            "Python: nombre no definido (NameError).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿typo en el nombre de la variable/función? ¿la definiste \
                 después de usarla? ¿faltó un import?".to_string(),
            ],
        )
    } else if output.contains("SyntaxError") || output.contains("IndentationError") {
        (
            "Python: error de sintaxis o indentación.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: revisá que no haya mezcla de tabs y espacios, o que no \
                 falten dos puntos (`:`) al final de una línea de bloque.".to_string(),
            ],
        )
    } else if output.contains("ValueError") {
        (
            "Python: valor inválido (ValueError).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: el tipo es correcto pero el valor no (ej: conversión \
                 fallida, argumento fuera de rango). Revisá el dato concreto.".to_string(),
            ],
        )
    } else if output.contains("KeyError") {
        (
            "Python: clave de diccionario no encontrada (KeyError).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿la clave existe siempre? Usá `.get()` en vez de `[]` si \
                 puede faltar, o validá antes.".to_string(),
            ],
        )
    } else if output.contains("FileNotFoundError") {
        (
            "Python: archivo no encontrado (FileNotFoundError).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: ¿el path es correcto? ¿es relativo y lo estás corriendo \
                 desde otro directorio? Probá con pathlib.Path absoluto.".to_string(),
            ],
        )
    } else {
        (
            "Python lanzó una excepción (revisá el traceback arriba para el tipo \
             exacto).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: el traceback muestra archivo, línea y excepción; empezá \
                 por la última línea (el error concreto) y subí hasta tu código.".to_string(),
            ],
        )
    };

    Some(DiagnosticReport { hint, suggestions })
}

/// Extrae "src/module.py:42" del traceback de Python.
/// Busca la ÚLTIMA línea `File "path", line N` (la más cercana al error real).
fn extract_python_location(output: &str) -> String {
    let mut last = None;
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("File \"") {
            if let Some(rest) = t.strip_prefix("File \"") {
                if let Some((path, rest)) = rest.split_once('"') {
                    if let Some(line_num) = rest
                        .trim_start_matches(", line ")
                        .split(',')
                        .next()
                        .and_then(|n| n.trim().parse::<u32>().ok())
                    {
                        last = Some(format!("{path}:{line_num}"));
                    }
                }
            }
        }
    }
    last.unwrap_or_else(|| "src/main.py".to_string())
}

// ═══════════════════════════════════════════════════════════════════════════
// Java / JVM: javac, Maven, Gradle
// ═══════════════════════════════════════════════════════════════════════════

fn diagnose_java(output: &str) -> Option<DiagnosticReport> {
    let out_lower = output.to_lowercase();

    // ¿Huele a Java? Maven/Gradle/javac tienen marcadores claros.
    let is_java = out_lower.contains("mvn") || out_lower.contains("maven")
        || out_lower.contains("gradle") || out_lower.contains("javac")
        || out_lower.contains("spring boot") || out_lower.contains(".java")
        || out_lower.contains("build failure") || out_lower.contains("build failed")
        || out_lower.contains("could not resolve dependencies")
        || out_lower.contains("[error]");

    if !is_java {
        return None;
    }

    let file_line = extract_java_location(output);

    let (hint, suggestions) = if out_lower.contains("already defined")
        || out_lower.contains("duplicate class")
    {
        (
            "Clase o componente duplicado en Java.".to_string(),
            vec![
                format!("dpx:search pattern={}", extract_class_name(output)),
                "Pista: dos archivos .java declaran la misma clase, o hay dos beans \
                 de Spring con el mismo nombre. Revisá @Component, @Service, @Bean.".to_string(),
            ],
        )
    } else if out_lower.contains("cannot find symbol") {
        (
            "Java: no encuentra un símbolo (clase, método, variable).".to_string(),
            vec![
                read_hint(&file_line),
                "¿Falta un import? ¿La clase está en otro paquete? Revisá el \
                 pom.xml/build.gradle por la dependencia correcta.".to_string(),
            ],
        )
    } else if out_lower.contains("incompatible types") {
        (
            "Java: tipos incompatibles en una asignación o llamada.".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: el compilador te dice qué tipo esperaba y qué recibió. \
                 ¿Falta un cast o un map() en un genérico?".to_string(),
            ],
        )
    } else if out_lower.contains("schemamanagementexception")
        || out_lower.contains("table not found")
    {
        (
            "Problema de esquema de base de datos: migraciones no aplicadas o \
             tablas inexistentes.".to_string(),
            vec![
                "dpx:read path=src/main/resources/application.yml".to_string(),
                "Pista: revisá Flyway/Liquibase, o si usás ddl-auto, asegurate de \
                 que las entidades mapeen correctamente.".to_string(),
            ],
        )
    } else if out_lower.contains("classnotfoundexception")
        || out_lower.contains("noclassdeffounderror")
    {
        (
            "Falta una dependencia o clase en el classpath.".to_string(),
            vec![
                "dpx:read path=pom.xml".to_string(),
                "Pista: ¿la dependencia está declarada con el scope correcto? \
                 ¿runtime vs compile vs test?".to_string(),
            ],
        )
    } else if out_lower.contains("package ")
        && out_lower.contains("does not exist")
    {
        (
            "Java: no encuentra un paquete entero (problema de dependencias o naming).".to_string(),
            vec![
                "dpx:read path=pom.xml".to_string(),
                read_hint(&file_line),
                "Pista: ¿la dependencia está en el pom? ¿el groupId/artifactId \
                 es correcto? ¿hace falta agregar un módulo o repository?".to_string(),
            ],
        )
    } else if out_lower.contains("could not resolve dependencies") {
        (
            "Maven/Gradle no pudo resolver dependencias.".to_string(),
            vec![
                "dpx:read path=pom.xml".to_string(),
                "Pista: ¿conexión a internet? ¿el repositorio es accesible? \
                 ¿la versión de la dependencia existe? Probá con `mvn -U compile`.".to_string(),
            ],
        )
    } else if out_lower.contains("build failure") || out_lower.contains("build failed") {
        (
            "Falló el build de Java (Maven/Gradle).".to_string(),
            vec![
                read_hint(&file_line),
                "Pista: subí en la salida hasta el primer [ERROR] para ver la \
                 causa raíz; los errores siguientes suelen ser consecuencias.".to_string(),
            ],
        )
    } else {
        return None;
    };

    Some(DiagnosticReport { hint, suggestions })
}

/// Extrae "src/main/java/App.java:42" de salidas javac/Maven/Gradle.
fn extract_java_location(output: &str) -> String {
    // Maven: `[ERROR] /path/to/File.java:[42,10] cannot find symbol`
    for line in output.lines() {
        let t = line.trim();
        if t.contains(".java") {
            if let Some(rest) = t.strip_prefix("[ERROR] ") {
                if let Some((path, rest)) = rest.split_once(".java:") {
                    let rest = rest.trim_start_matches('[');
                    if let Some((row, _)) = rest.split_once(|c: char| c == ',' || c == ']') {
                        return format!("{path}.java:{row}");
                    }
                }
            }
            // Gradle: `src/main/java/App.java:42: error: ...`
            if let Some((path, rest)) = t.split_once(".java:") {
                let row = rest
                    .split(|c: char| c == ':' || c == ' ')
                    .next()
                    .unwrap_or("0");
                return format!("{path}.java:{row}");
            }
        }
    }
    // Fallback: buscar cualquier path con .java
    for line in output.lines() {
        if let Some(start) = line.find(".java") {
            let end = start + 5;
            let before = &line[..end];
            if let Some(path_start) = before.rfind(|c: char| c == ' ' || c == '\t') {
                let path = before[path_start..].trim();
                return path.to_string();
            }
        }
    }
    "src/main/java".to_string()
}

/// Extrae un nombre de clase simple de la salida de error.
fn extract_class_name(output: &str) -> String {
    output
        .split("class ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .map(|c| c.trim_matches(|ch: char| !ch.is_alphanumeric()))
        .filter(|c| !c.is_empty())
        .unwrap_or("NOMBRE_DE_LA_CLASE")
        .to_string()
}

// ═══════════════════════════════════════════════════════════════════════════
// Genérico: fallback cuando ningún detector específico casa
// ═══════════════════════════════════════════════════════════════════════════

fn diagnose_generic(output: &str) -> Option<DiagnosticReport> {
    let out_lower = output.to_lowercase();

    // Solo si hay indicios claros de error.
    let has_error = out_lower.contains("error")
        || out_lower.contains("fail")
        || out_lower.contains("exception");
    if !has_error {
        return None;
    }

    let paths = extract_paths(output);
    let mut suggestions: Vec<String> = paths
        .into_iter()
        .map(|p| format!("dpx:read path={p}"))
        .collect();

    if suggestions.is_empty() {
        suggestions.push(
            "dpx:search pattern=ERROR".to_string(),
        );
    }

    Some(DiagnosticReport {
        hint: "Parece que hubo un error, pero no reconocí el lenguaje exacto. \
               Revisá los archivos sugeridos para encontrar el problema.".to_string(),
        suggestions,
    })
}

/// Extrae rutas de archivo mencionadas en la salida (heurística).
fn extract_paths(output: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let extensions = [".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".java",
                      ".kt", ".yaml", ".yml", ".toml", ".json", ".css",
                      ".html", ".xml", ".gradle", ".sql", ".md"];
    for line in output.lines() {
        let t = line.trim();
        // Path absoluto o relativo con extensión conocida
        for ext in &extensions {
            if let Some(idx) = t.find(ext) {
                let end = idx + ext.len();
                // Buscar el inicio del path hacia atrás
                let start = t[..idx]
                    .rfind(|c: char| matches!(c, ' ' | '\t' | '(' | '[' | '"' | '\''))
                    .map(|p| p + 1)
                    .unwrap_or(0);
                let path = &t[start..end];
                if path.len() > 2 && (path.contains('/') || path.contains('\\')) {
                    let candidate = path.to_string();
                    if !paths.contains(&candidate) {
                        paths.push(candidate);
                    }
                }
                break;
            }
        }
    }
    paths.truncate(5);
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Rust ────────────────────────────────────────────────────────────

    #[test]
    fn rust_e0597_lifetime() {
        let output = "error[E0597]: `x` does not live long enough
  --> src/main.rs:15:10
   |
15 |     let y = &x;
   |             ^^ borrowed value does not live long enough";
        let diag = diagnose_failure(output).expect("debió detectar Rust");
        assert!(diag.hint.contains("E0597"));
        assert!(diag.suggestions.iter().any(|s| s.contains("path=src/main.rs") && s.contains("15")));
    }

    #[test]
    fn rust_e0502_borrow_conflict() {
        let output = "error[E0502]: cannot borrow `v` as mutable because it is also borrowed as immutable
  --> src/lib.rs:42:5";
        let diag = diagnose_failure(output).expect("debió detectar Rust");
        assert!(diag.hint.contains("E0502"));
        assert!(diag.suggestions.iter().any(|s| s.contains("path=src/lib.rs") && s.contains("42")));
    }

    #[test]
    fn rust_e0308_type_mismatch() {
        let output = "error[E0308]: mismatched types
  --> src/handler.rs:30:22
   |
30 |     process(input);
   |            ^^^^^ expected String, found &str";
        let diag = diagnose_failure(output).expect("debió detectar Rust");
        assert!(diag.hint.contains("E0308"));
    }

    #[test]
    fn rust_e0432_unresolved_import() {
        let output = "error[E0432]: unresolved import `crate::nonexistent::Module`
 --> src/foo.rs:3:5";
        let diag = diagnose_failure(output).expect("debió detectar Rust");
        assert!(diag.hint.contains("E0432"));
    }

    #[test]
    fn read_hint_separa_path_de_linea() {
        // El path debe quedar LEÍBLE (sin la línea pegada, que rompería read_file).
        let h = read_hint("src/main.rs:142");
        assert!(h.contains("path=src/main.rs "), "el path no debe llevar :142 pegado");
        assert!(h.contains("línea 142"));
        // Sin número de línea: se deja tal cual.
        assert_eq!(read_hint("pom.xml"), "dpx:read path=pom.xml");
    }

    #[test]
    fn sugerencia_rust_no_pega_la_linea_al_path() {
        let output = "error[E0308]: mismatched types\n  --> src/handler.rs:30:22\n";
        let diag = diagnose_failure(output).unwrap();
        // Ninguna sugerencia debe pedir leer "path=...:30" (ruta inválida).
        assert!(diag.suggestions.iter().all(|s| !s.contains("handler.rs:30")));
        assert!(diag.suggestions.iter().any(|s| s.contains("path=src/handler.rs")));
    }

    #[test]
    fn rust_file_location_extrae_bien() {
        // El extractor quita la columna y deja path:línea
        let loc = extract_rust_location(
            "error[E0001]: boom\n  --> src/network/client.rs:142:10\n",
        );
        assert_eq!(loc, "src/network/client.rs:142");
    }

    #[test]
    fn rust_no_error_es_none() {
        assert!(diagnose_rust("Compiling dpx v0.2.0\n   Finished dev").is_none());
    }

    // ── TypeScript ──────────────────────────────────────────────────────

    #[test]
    fn ts_ts2345_wrong_arg_type() {
        let output = "src/api.ts(15,22): error TS2345: Argument of type 'string' is not assignable to parameter of type 'number'.";
        let diag = diagnose_failure(output).expect("debió detectar TS");
        assert!(diag.hint.contains("TS2345"));
        assert!(diag.suggestions.iter().any(|s| s.contains("path=src/api.ts") && s.contains("15")));
    }

    #[test]
    fn ts_ts2339_property_not_found() {
        let output = "src/card.tsx(42,10): error TS2339: Property 'onClick' does not exist on type 'Props'.";
        let diag = diagnose_failure(output).expect("debió detectar TS");
        assert!(diag.hint.contains("TS2339"));
    }

    #[test]
    fn ts_ts2304_cannot_find_name() {
        let output = "src/utils.ts(8,5): error TS2304: Cannot find name 'prisma'.";
        let diag = diagnose_failure(output).expect("debió detectar TS");
        assert!(diag.hint.contains("TS2304"));
    }

    #[test]
    fn ts_location_extrae_bien() {
        let loc = extract_ts_location(
            "src/components/nav.tsx(142,10): error TS2322: ...",
        );
        assert_eq!(loc, "src/components/nav.tsx:142");
    }

    // ── Python ──────────────────────────────────────────────────────────

    #[test]
    fn py_module_not_found() {
        let output = "Traceback (most recent call last):
  File \"src/app.py\", line 3, in <module>
    from services.auth import verify_token
ModuleNotFoundError: No module named 'services'";
        let diag = diagnose_failure(output).expect("debió detectar Python");
        assert!(diag.hint.contains("ModuleNotFoundError"));
        assert!(diag.suggestions.iter().any(|s| s.contains("path=src/app.py") && s.contains("3")));
    }

    #[test]
    fn py_attribute_error() {
        let output = "Traceback (most recent call last):
  File \"src/worker.py\", line 55, in run
    result = client.send_message(payload)
AttributeError: 'NoneType' object has no attribute 'send_message'";
        let diag = diagnose_failure(output).expect("debió detectar Python");
        assert!(diag.hint.contains("AttributeError"));
        assert!(diag.suggestions.iter().any(|s| s.contains("path=src/worker.py") && s.contains("55")));
    }

    #[test]
    fn py_syntax_error() {
        let output = "  File \"src/parser.py\", line 22
    def parse(data)
                  ^
SyntaxError: expected ':'";
        let diag = diagnose_failure(output).expect("debió detectar Python");
        assert!(diag.hint.contains("sintaxis"));
    }

    #[test]
    fn py_toma_la_ultima_linea_del_traceback() {
        // Varias líneas File; la última es la más cercana al error real.
        let output = "Traceback (most recent call last):
  File \"src/main.py\", line 10, in <module>
    app.run()
  File \"src/services/handler.py\", line 42, in run
    raise ValueError('config missing')
ValueError: config missing";
        let loc = extract_python_location(output);
        assert_eq!(loc, "src/services/handler.py:42");
    }

    #[test]
    fn py_type_error() {
        let output = "Traceback (most recent call last):
  File \"src/calc.py\", line 8, in <module>
    total = price + tax
TypeError: can only concatenate str (not \"int\") to str";
        let diag = diagnose_failure(output).expect("debió detectar Python");
        assert!(diag.hint.contains("TypeError"));
    }

    // ── Java ────────────────────────────────────────────────────────────

    #[test]
    fn java_cannot_find_symbol() {
        let output = "[ERROR] /home/project/src/main/java/com/app/UserController.java:[42,10] cannot find symbol
  symbol:   class UserService
  location: class com.app.UserController";
        let diag = diagnose_failure(output).expect("debió detectar Java");
        assert!(diag.hint.contains("símbolo"));
        assert!(diag.suggestions.iter().any(|s| s.contains("UserController.java") && s.contains("42")));
    }

    #[test]
    fn java_duplicate_class() {
        let output = "class AuthService already defined in src/main/java/com/app/AuthService.java";
        let diag = diagnose_failure(output).expect("debió detectar Java");
        assert!(diag.hint.contains("duplicado"));
    }

    #[test]
    fn java_incompatible_types() {
        let output = "[ERROR] incompatible types: String cannot be converted to int
  src/main/java/com/app/Counter.java:30: error: ...";
        let diag = diagnose_failure(output).expect("debió detectar Java");
        assert!(diag.hint.contains("incompatibles"));
    }

    #[test]
    fn java_package_does_not_exist() {
        let output = "[ERROR] package jakarta.validation does not exist";
        let diag = diagnose_failure(output).expect("debió detectar Java");
        assert!(diag.hint.contains("paquete"));
    }

    #[test]
    fn java_could_not_resolve_deps() {
        let output = "Could not resolve dependencies for project com.app:api:jar:1.0.0
  Could not find artifact org.springframework.boot:spring-boot-starter-web:jar:4.0.6";
        let diag = diagnose_failure(output).expect("debió detectar Java");
        assert!(diag.hint.contains("dependencias"));
    }

    // ── Genérico ────────────────────────────────────────────────────────

    #[test]
    fn generic_con_paths_extrae_archivos() {
        let output = "ERROR: something went wrong\n  at src/utils.rs:42\n  at src/lib.rs:10";
        let diag = diagnose_failure(output).expect("debió detectar genérico");
        assert!(diag.suggestions.len() >= 1);
        assert!(diag.suggestions.iter().any(|s| s.contains("utils.rs")));
    }

    #[test]
    fn texto_sin_errores_devuelve_none() {
        assert!(diagnose_failure("Compilation successful\nBUILD SUCCESS").is_none());
        assert!(diagnose_failure("All tests passed").is_none());
    }

    // ── Dispatch principal ──────────────────────────────────────────────

    #[test]
    fn dispatch_rust_tiene_prioridad() {
        let output = "error[E0308]: mismatched types
  --> src/main.rs:5:10
  ...";
        let diag = diagnose_failure(output).expect("debió detectar");
        assert!(diag.hint.contains("E0308"));
    }

    #[test]
    fn dispatch_ts_tras_rust() {
        let output = "src/app.ts(10,5): error TS2345: ...";
        let diag = diagnose_failure(output).expect("debió detectar");
        assert!(diag.hint.contains("TS2345"));
    }

    #[test]
    fn dispatch_python_tras_java_y_ts() {
        let output = "Traceback (most recent call last):
  File \"a.py\", line 1, in <module>
NameError: name 'x' is not defined";
        let diag = diagnose_failure(output).expect("debió detectar");
        assert!(diag.hint.contains("NameError"));
    }
}
