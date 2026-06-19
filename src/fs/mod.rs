//! Escritura de archivos al proyecto, con seguridad y bajo confirmación.
//!
//! El agente propone cambios vía tool calls nativas (`write_file`/`edit_file`/
//! `delete_file`); este módulo las aplica. El REPL muestra un preview y pide
//! confirmación antes de escribir nada. Nunca se escribe fuera del directorio
//! del proyecto. Los únicos bloques de texto que aún se parsean son `dpx:plan`
//! (checklist) y `dpx:skill` (progreso del modo learn), que no tienen
//! equivalente nativo.

pub mod safety;
mod detect;
mod edit;
mod exec;
mod grep;
mod tree;

// Submódulos extraídos; se re-exportan para conservar las rutas públicas
// `crate::fs::*` que usan los call sites (sin cambios fuera de aquí).
pub use detect::{
    build_manifest, detect_build, detect_stack, detect_test, edits_touch_build, touches_build,
};
pub use edit::*; // FileEdit, apply_edit
pub use exec::{RUN_TIMEOUT_SECS, run_command, run_command_streaming};
pub use grep::{orphan_refs, search_in_project};
pub use tree::{project_tree, symbol_map};

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow};

/// Una propuesta de archivo a escribir.
pub struct FileWrite {
    pub path: String,
    pub content: String,
}

impl FileWrite {
    /// Número de líneas del contenido propuesto.
    pub fn line_count(&self) -> usize {
        self.content.lines().count()
    }
}

/// ¿La info-string de un fence es un plan `dpx:plan`?
fn is_plan_fence(s: &str) -> bool {
    let t = s.trim();
    t == "dpx:plan" || t.starts_with("dpx:plan ")
}

/// Extrae el plan de un bloque `dpx:plan` (una tarea por línea, `[ ]` pendiente /
/// `[x]` hecha). Devuelve `(hecha, texto)` por ítem, o `None` si no hay plan.
pub fn parse_plan(text: &str) -> Option<Vec<(bool, String)>> {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let mut is_plan = is_plan_fence(info);
        if !is_plan && lines.peek().is_some_and(|n| is_plan_fence(n)) {
            is_plan = true;
            lines.next();
        }
        if !is_plan {
            continue;
        }
        let mut items = Vec::new();
        for body in lines.by_ref() {
            if body.trim_start().starts_with("```") {
                break;
            }
            let t = body.trim().trim_start_matches(['-', '*', ' ']);
            if t.is_empty() {
                continue;
            }
            if let Some(rest) = t.strip_prefix("[x]").or_else(|| t.strip_prefix("[X]")) {
                items.push((true, rest.trim().to_string()));
            } else if let Some(rest) = t.strip_prefix("[ ]").or_else(|| t.strip_prefix("[]")) {
                items.push((false, rest.trim().to_string()));
            } else {
                items.push((false, t.to_string()));
            }
        }
        if !items.is_empty() {
            return Some(items);
        }
    }
    None
}

/// Convierte un plan parseado (lista de pares hecho/texto) a formato Markdown
/// para persistencia entre sesiones.
pub fn plan_to_markdown(plan: &[(bool, String)]) -> String {
    let mut md = String::from("# Plan pendiente\n\n```dpx:plan\n");
    for (done, text) in plan {
        let marker = if *done { "[x]" } else { "[ ]" };
        md.push_str(&format!("{marker} {text}\n"));
    }
    md.push_str("```\n");
    md
}

/// Extrae el último plan de una lista de turnos de sesión (el más reciente
/// bloque `dpx:plan` emitido por el asistente).
pub fn extract_last_plan(turns: &[crate::session::Turn]) -> Option<Vec<(bool, String)>> {
    for turn in turns.iter().rev() {
        if turn.role == "assistant"
            && let Some(plan) = parse_plan(&turn.text) {
                return Some(plan);
            }
    }
    None
}

/// Conserva solo las últimas `max_lines` líneas (los errores de build suelen estar al final).
fn cap_tail(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() > max_lines {
        format!(
            "…[salida truncada, últimas {max_lines} líneas]\n{}",
            lines[lines.len() - max_lines..].join("\n")
        )
    } else {
        s.to_string()
    }
}

/// Quita del texto los bloques `dpx:plan` y `dpx:skill` para poder renderizar
/// solo la prosa como Markdown. Los bloques de código normales se conservan.
pub fn strip_action_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            let on_fence = is_plan_fence(info) || crate::skill::is_skill_fence(info);
            let on_next = lines
                .peek()
                .is_some_and(|n| is_plan_fence(n) || crate::skill::is_skill_fence(n));
            if on_fence || on_next {
                // Saltar el bloque entero hasta el cierre ```.
                for body in lines.by_ref() {
                    if body.trim_start().starts_with("```") {
                        break;
                    }
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Resuelve la ruta destino DENTRO del proyecto, rechazando rutas absolutas o
/// que escapen del directorio con `..`.
pub fn safe_target(project_root: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(anyhow!("ruta absoluta no permitida: {rel}"));
    }
    for comp in rel_path.components() {
        match comp {
            Component::ParentDir => return Err(anyhow!("la ruta no puede salir del proyecto: {rel}")),
            Component::Prefix(_) | Component::RootDir => {
                return Err(anyhow!("ruta no permitida: {rel}"));
            }
            _ => {}
        }
    }
    Ok(project_root.join(rel_path))
}

/// Escribe el archivo en disco, creando los directorios padres necesarios.
pub fn apply(project_root: &Path, write: &FileWrite) -> Result<PathBuf> {
    let target = safe_target(project_root, &write.path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| anyhow!("no pude crear {}: {e}", parent.display()))?;
    }
    fs::write(&target, &write.content)
        .map_err(|e| anyhow!("no pude escribir {}: {e}", target.display()))?;
    Ok(target)
}

/// ¿El archivo destino ya existe? (para avisar de sobrescritura en el preview)
pub fn exists(project_root: &Path, rel: &str) -> bool {
    safe_target(project_root, rel).map(|p| p.exists()).unwrap_or(false)
}

/// Contenido actual del archivo (sin tope de líneas), o `None` si no existe o no
/// se puede leer. Sirve de base para el diff del preview de escritura.
pub fn current_content(project_root: &Path, rel: &str) -> Option<String> {
    let target = safe_target(project_root, rel).ok()?;
    fs::read_to_string(target).ok()
}

/// Tope de líneas por lectura para no inundar el contexto. Generoso a propósito:
/// un archivo que el modelo va a EDITAR debe verse entero.
const READ_MAX_LINES: usize = 2500;

/// Lee un archivo del proyecto (para referencias `@archivo` y el subagente),
/// desde el principio y con el tope por defecto.
pub fn read_file(project_root: &Path, rel: &str) -> Result<String> {
    read_file_range(project_root, rel, None, None)
}

/// Lee un archivo con soporte de RANGO: `offset` = línea inicial (1-based),
/// `limit` = nº máximo de líneas. Para archivos más largos que el tope, en vez
/// de cortar a ciegas le dice al modelo cuántas líneas faltan y con qué `offset`
/// pedirlas — así puede leer el FINAL de un archivo grande con su propia
/// herramienta (read_file), sin inventar scripts para hacer `tail`.
pub fn read_file_range(
    project_root: &Path,
    rel: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String> {
    let target = safe_target(project_root, rel)?;
    let data = fs::read_to_string(&target)
        .map_err(|e| anyhow!("no pude leer {}: {e}", target.display()))?;

    let all: Vec<&str> = data.lines().collect();
    let total = all.len();
    let start = offset.unwrap_or(1).max(1); // 1-based
    let limit = limit.unwrap_or(READ_MAX_LINES);

    // Lectura por defecto de un archivo que cabe entero: devuelve el contenido
    // EXACTO (preserva el salto final), como antes.
    if start == 1 && total <= limit {
        return Ok(data);
    }
    if start > total {
        return Ok(format!(
            "[el archivo `{rel}` tiene {total} líneas; pediste desde la {start}, que no existe]"
        ));
    }

    let start_idx = start - 1;
    let end_idx = (start_idx + limit).min(total);
    let slice = all[start_idx..end_idx].join("\n");
    let mut out = format!("[`{rel}` líneas {start}–{end_idx} de {total}]\n{slice}");
    if end_idx < total {
        out.push_str(&format!(
            "\n…[faltan {} líneas; para verlas vuelve a llamar a read_file con offset={}]",
            total - end_idx,
            end_idx + 1
        ));
    }
    Ok(out)
}

/// Borra un archivo de forma segura.
pub fn delete_file(project_root: &Path, rel: &str) -> Result<()> {
    let target = safe_target(project_root, rel)?;
    if target.exists() {
        fs::remove_file(&target)
            .map_err(|e| anyhow::anyhow!("no pude borrar {}: {}", target.display(), e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touches_build_detecta_fuente_y_manifiesto() {
        let java = vec![FileWrite { path: "src/main/java/com/app/Foo.java".into(), content: String::new() }];
        let pom = vec![FileWrite { path: "pom.xml".into(), content: String::new() }];
        let rust = vec![FileWrite { path: "src/cli/chat.rs".into(), content: String::new() }];
        let doc = vec![FileWrite { path: "README.md".into(), content: String::new() }];
        assert!(touches_build(&java));
        assert!(touches_build(&pom));
        assert!(touches_build(&rust));
        assert!(!touches_build(&doc));
    }

    #[test]
    fn rechaza_rutas_inseguras() {
        let root = Path::new("/proj");
        assert!(safe_target(root, "../escape.txt").is_err());
        assert!(safe_target(root, "ok/inside.txt").is_ok());
    }

    #[test]
    fn run_command_basico_entrega_lineas_y_exit_code() {
        let dir = std::env::temp_dir();
        let mut lines = Vec::new();
        let res = run_command_streaming(
            &dir,
            "echo hola",
            30,
            &mut |l| lines.push(l.to_string()),
            &|| false,
        );
        assert!(!res.cancelled);
        assert!(res.output.contains("exit code: 0"));
        // Fuente de verdad del green-gate: el código de salida REAL, no el texto.
        assert_eq!(res.exit_code, Some(0));
        assert!(lines.iter().any(|l| l.contains("hola")));
    }

    #[test]
    fn run_command_fallido_expone_exit_code_distinto_de_cero() {
        // El green-gate distingue verde (Some(0)) de rojo por el código REAL,
        // que sobrevive aunque `cap_tail` recorte la línea `exit code:` en una
        // salida larga. Un comando que sale con error debe reportarse en rojo.
        let dir = std::env::temp_dir();
        let fail = if cfg!(windows) { "cmd /c exit 1" } else { "sh -c 'exit 1'" };
        let res = run_command_streaming(&dir, fail, 30, &mut |_| {}, &|| false);
        assert!(!res.cancelled);
        assert_ne!(res.exit_code, Some(0), "un fallo no puede pasar por verde");
        assert_eq!(res.exit_code, Some(1));
    }

    #[test]
    fn run_command_corta_por_timeout() {
        let dir = std::env::temp_dir();
        let slow = if cfg!(windows) { "ping -n 10 127.0.0.1 > NUL" } else { "sleep 10" };
        let t = std::time::Instant::now();
        let res = run_command_streaming(&dir, slow, 1, &mut |_| {}, &|| false);
        assert!(res.output.contains("TIMEOUT"));
        assert!(!res.cancelled);
        assert!(t.elapsed().as_secs() < 8);
    }

    #[test]
    fn run_command_cancelado_mata_el_proceso() {
        let dir = std::env::temp_dir();
        let slow = if cfg!(windows) { "ping -n 10 127.0.0.1 > NUL" } else { "sleep 10" };
        let t = std::time::Instant::now();
        let res = run_command_streaming(&dir, slow, 30, &mut |_| {}, &|| true);
        assert!(res.cancelled);
        assert!(res.output.contains("interrumpido"));
        assert!(t.elapsed().as_secs() < 8);
    }

    #[test]
    fn detect_stack_prioridades() {
        let dir = std::env::temp_dir().join(format!("dpx-detect-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(detect_stack(&dir), None);

        fs::write(dir.join("requirements.txt"), "").unwrap();
        assert_eq!(detect_stack(&dir), Some("python"));

        fs::write(dir.join("package.json"), r#"{"dependencies":{"express":"^4"}}"#).unwrap();
        assert_eq!(detect_stack(&dir), Some("node"));

        fs::write(dir.join("package.json"), r#"{"dependencies":{"react":"^18"}}"#).unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));

        fs::write(dir.join("pom.xml"), "<project/>").unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_build_y_test_por_stack() {
        let dir = std::env::temp_dir().join(format!("dpx-verify-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Sin manifiesto reconocible: nada que verificar.
        assert_eq!(detect_build(&dir), None);
        assert_eq!(detect_test(&dir), None);

        // Cargo: build = clippy estricto (check + deniega lints/dead-code),
        // test = la suite completa.
        fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let build = detect_build(&dir).unwrap();
        assert!(build.contains("clippy") && build.contains("-D warnings"), "build: {build}");
        assert_eq!(detect_test(&dir).as_deref(), Some("cargo test --quiet"));
        fs::remove_file(dir.join("Cargo.toml")).unwrap();

        // Maven sin wrapper: compile salta tests; test los corre.
        fs::write(dir.join("pom.xml"), "<project/>").unwrap();
        assert!(detect_build(&dir).unwrap().contains("-DskipTests"));
        assert!(detect_test(&dir).unwrap().contains("test"));
        assert!(!detect_test(&dir).unwrap().contains("-DskipTests"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_file_range_lee_el_final_de_un_archivo_grande() {
        let dir = std::env::temp_dir().join(format!("dpx-read-range-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Archivo de 3000 líneas: "linea N" en cada una.
        let body: String = (1..=3000).map(|n| format!("linea {n}\n")).collect();
        fs::write(dir.join("big.txt"), &body).unwrap();

        // Lectura por defecto (sin rango): trunca al tope y dice cómo seguir.
        let head = read_file_range(&dir, "big.txt", None, None).unwrap();
        assert!(head.contains("linea 1\n"));
        assert!(!head.contains("linea 3000"), "no debe llegar al final por defecto");
        assert!(head.contains("offset="), "debe indicar con qué offset seguir");

        // Lectura del FINAL con offset: lo que dpx necesitaba (en vez de un .py).
        let tail = read_file_range(&dir, "big.txt", Some(2900), Some(500)).unwrap();
        assert!(tail.contains("linea 2900"));
        assert!(tail.contains("linea 3000"), "el final debe estar accesible por offset");
        assert!(tail.contains("líneas 2900–3000 de 3000"));

        // Archivo pequeño leído entero: contenido exacto, sin cabecera de rango.
        fs::write(dir.join("small.txt"), "uno\ndos\n").unwrap();
        let small = read_file_range(&dir, "small.txt", None, None).unwrap();
        assert_eq!(small, "uno\ndos\n");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_maven_spring_boot_por_parent_pom() {
        let dir = std::env::temp_dir().join(format!("dpx-mvn-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pom.xml"),
            r#"<project>
  <parent>
    <groupId>org.springframework.boot</groupId>
    <artifactId>spring-boot-starter-parent</artifactId>
    <version>3.2.0</version>
  </parent>
</project>"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_maven_spring_boot_por_plugin() {
        let dir = std::env::temp_dir().join(format!("dpx-mvn-pl-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pom.xml"),
            r#"<project>
  <build>
    <plugins>
      <plugin>
        <groupId>org.springframework.boot</groupId>
        <artifactId>spring-boot-maven-plugin</artifactId>
      </plugin>
    </plugins>
  </build>
</project>"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_maven_simple_xml_sin_spring_cae_a_shallow() {
        // Nombre único: este dir NO puede colisionar con el de otro test que
        // corre en paralelo (mismo pid) — la causa del fallo flaky original.
        let dir = std::env::temp_dir().join(format!("dpx-mvn-simple-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pom.xml"),
            r#"<project>
  <groupId>com.test</groupId>
  <artifactId>simple</artifactId>
</project>"#,
        )
        .unwrap();
        // shallow detecta pom.xml → spring-boot (fallback genérico para JVM)
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_gradle_spring_boot_por_plugin() {
        let dir = std::env::temp_dir().join(format!("dpx-gradle-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("build.gradle"),
            r#"plugins {
  id 'org.springframework.boot' version '3.2.0'
}
dependencies {
  implementation 'org.springframework.boot:spring-boot-starter-web'
}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_gradle_sin_spring_cae_a_shallow() {
        let dir = std::env::temp_dir().join(format!("dpx-gradle-2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("build.gradle"), "apply plugin: 'java'").unwrap();
        // shallow detecta build.gradle → gradle
        assert_eq!(detect_stack(&dir), Some("gradle"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_next_js_como_react() {
        let dir = std::env::temp_dir().join(format!("dpx-next-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"next":"^14","react":"^18"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_gatsby_como_react() {
        let dir = std::env::temp_dir().join(format!("dpx-gatsby-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"gatsby":"^5"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_react_por_devdeps() {
        let dir = std::env::temp_dir().join(format!("dpx-rdep-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"devDependencies":{"react":"^18"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_peer_deps_reconocidas() {
        let dir = std::env::temp_dir().join(format!("dpx-peer-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"peerDependencies":{"react":"^18"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plan_to_markdown_redondea_correctamente() {
        let plan = vec![
            (true, "Migrar a jakarta.*".to_string()),
            (false, "Añadir validación".to_string()),
            (false, "Escribir tests".to_string()),
        ];
        let md = plan_to_markdown(&plan);
        assert!(md.contains("[x] Migrar a jakarta.*"));
        assert!(md.contains("[ ] Añadir validación"));
        assert!(md.contains("[ ] Escribir tests"));
        assert!(md.contains("```dpx:plan"));
    }

    #[test]
    fn extract_last_plan_encuentra_el_mas_reciente() {
        let turns = vec![
            crate::session::Turn { role: "assistant", text: "```dpx:plan\n[x] Tarea 1\n```".into() },
            crate::session::Turn { role: "user", text: "ok".into() },
            crate::session::Turn { role: "assistant", text: "```dpx:plan\n[ ] Tarea 2\n```".into() },
        ];
        let plan = extract_last_plan(&turns).unwrap();
        assert_eq!(plan.len(), 1);
        assert!(!plan[0].0);
        assert_eq!(plan[0].1, "Tarea 2");
    }

    #[test]
    fn extract_last_plan_sin_plan_devuelve_none() {
        let turns = vec![
            crate::session::Turn { role: "assistant", text: "texto normal".into() },
            crate::session::Turn { role: "user", text: "hola".into() },
        ];
        assert!(extract_last_plan(&turns).is_none());
    }
}
