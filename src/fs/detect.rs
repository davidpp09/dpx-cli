//! Detección de build/test y de stack del proyecto (Maven/Gradle/Cargo/npm/
//! Python), más el manifiesto real para grounding. Extraído de `fs`.

use std::fs;
use std::path::Path;

use super::{FileEdit, FileWrite};

/// Detecta el comando de build del proyecto (Maven, Gradle o Cargo) para la
/// verificación automática tras escribir código. `None` si no hay build
/// reconocible.
pub fn detect_build(cwd: &Path) -> Option<String> {
    if cwd.join("pom.xml").exists() {
        // Prefiere el Maven Wrapper del proyecto (no requiere Maven global instalado).
        let wrapper = if cfg!(windows) { "mvnw.cmd" } else { "mvnw" };
        if cwd.join(wrapper).exists() {
            let invoke = if cfg!(windows) { "mvnw.cmd" } else { "./mvnw" };
            return Some(format!("{invoke} -q -DskipTests compile"));
        }
        return Some("mvn -q -DskipTests compile".to_string());
    }
    if cwd.join("build.gradle").exists() || cwd.join("build.gradle.kts").exists() {
        let wrapper = if cfg!(windows) { "gradlew.bat" } else { "gradlew" };
        if cwd.join(wrapper).exists() {
            let invoke = if cfg!(windows) { "gradlew.bat" } else { "./gradlew" };
            return Some(format!("{invoke} compileJava -q"));
        }
        return Some("gradle compileJava -q".to_string());
    }
    if cwd.join("Cargo.toml").exists() {
        // clippy en vez de `cargo check`: implica el check Y además deniega
        // warnings (código muerto, lints) — lo que `cargo check`/`cargo test`
        // NO hacen. Es lo que evita que dpx cante "listo" dejando dead-code que
        // luego revienta el CI (`-D warnings`).
        return Some("cargo clippy --quiet --all-targets -- -D warnings".to_string());
    }
    None
}

/// Detecta el comando de tests del proyecto (Maven, Gradle o Cargo). A
/// diferencia de [`detect_build`], esto COMPILA y además corre la suite: lo usa
/// el modo full-auto del agente para verificar de verdad tras escribir código
/// (compila → prueba → se autocorrige), no solo que compile. `None` si no se
/// reconoce el stack.
pub fn detect_test(cwd: &Path) -> Option<String> {
    if cwd.join("pom.xml").exists() {
        let wrapper = if cfg!(windows) { "mvnw.cmd" } else { "mvnw" };
        if cwd.join(wrapper).exists() {
            let invoke = if cfg!(windows) { "mvnw.cmd" } else { "./mvnw" };
            return Some(format!("{invoke} -q test"));
        }
        return Some("mvn -q test".to_string());
    }
    if cwd.join("build.gradle").exists() || cwd.join("build.gradle.kts").exists() {
        let wrapper = if cfg!(windows) { "gradlew.bat" } else { "gradlew" };
        if cwd.join(wrapper).exists() {
            let invoke = if cfg!(windows) { "gradlew.bat" } else { "./gradlew" };
            return Some(format!("{invoke} test -q"));
        }
        return Some("gradle test -q".to_string());
    }
    if cwd.join("Cargo.toml").exists() {
        return Some("cargo test --quiet".to_string());
    }
    None
}

/// Detecta el stack del proyecto mirando los archivos de la raíz.
///
/// Primero intenta detección profunda (leyendo dependencias reales de los
/// manifiestos: pom.xml, build.gradle, Cargo.toml, package.json, etc.).
/// Si no reconoce nada, cae en detección por tipo de archivo de build.
/// `None` si no se reconoce ninguno (mentor genérico, sin skills de dominio).
pub fn detect_stack(cwd: &Path) -> Option<&'static str> {
    detect_stack_deep(cwd).or_else(|| detect_stack_shallow(cwd))
}

/// Detección profunda: parsea manifiestos de dependencias reales.
///
/// Mantiene el mismo orden de prioridad que la shallow: JVM (Maven/Gradle)
/// antes que Node.js, Rust y Python. Si un proyecto tiene pom.xml Y
/// package.json (ej. frontend+backend mono-repo), JVM gana.
fn detect_stack_deep(cwd: &Path) -> Option<&'static str> {
    // JVM: Maven primero, Gradle después
    if cwd.join("pom.xml").exists() {
        if let Some(stack) = detect_maven_stack(cwd) {
            return Some(stack);
        }
        // pom.xml sin Spring Boot → JVM genérico → spring-boot pack
        return Some("spring-boot");
    }
    if cwd.join("build.gradle").exists() || cwd.join("build.gradle.kts").exists() {
        if let Some(stack) = detect_gradle_stack(cwd) {
            return Some(stack);
        }
        return Some("gradle");
    }
    // Node.js
    if let Some(stack) = detect_npm_stack(cwd) {
        return Some(stack);
    }
    // Rust
    if cwd.join("Cargo.toml").exists() {
        // dpx editándose a sí mismo → pack de auto-edición (su arquitectura + UI).
        return Some(if is_dpx_repo(cwd) { "dpx" } else { "rust" });
    }
    // Python
    if let Some(stack) = detect_python_stack(cwd) {
        return Some(stack);
    }
    None
}

/// ¿El proyecto abierto es el propio repositorio de dpx? Se reconoce por el
/// nombre del paquete en `Cargo.toml` (`name = "dpx-cli"`). Sirve para cargar
/// el focus pack de auto-edición cuando dpx trabaja sobre su propio código.
fn is_dpx_repo(cwd: &Path) -> bool {
    let Ok(data) = fs::read_to_string(cwd.join("Cargo.toml")) else {
        return false;
    };
    // Busca `name = "dpx-cli"` dentro de la sección [package] (tolerante a
    // espacios y comillas simples/dobles), sin parsear TOML entero.
    data.lines().any(|line| {
        let l = line.trim();
        l.starts_with("name") && l.contains("dpx-cli")
    })
}

/// Detección superficial por tipo de archivo de build (fallback).
fn detect_stack_shallow(cwd: &Path) -> Option<&'static str> {
    if cwd.join("pom.xml").exists() || cwd.join("mvnw").exists() || cwd.join("mvnw.cmd").exists() {
        return Some("spring-boot");
    }
    if cwd.join("package.json").exists() {
        return Some(if package_json_has_react(cwd) { "react" } else { "node" });
    }
    if cwd.join("Cargo.toml").exists() {
        return Some(if is_dpx_repo(cwd) { "dpx" } else { "rust" });
    }
    if cwd.join("build.gradle").exists() || cwd.join("build.gradle.kts").exists() {
        return Some("gradle");
    }
    if cwd.join("requirements.txt").exists() || cwd.join("pyproject.toml").exists() {
        return Some("python");
    }
    None
}

/// Detecta stack desde pom.xml: busca Spring Boot, Quarkus, Micronaut.
fn detect_maven_stack(cwd: &Path) -> Option<&'static str> {
    let path = cwd.join("pom.xml");
    if !path.exists() {
        return None;
    }
    let data = fs::read_to_string(path).ok()?;
    // Spring Boot: parent POM (más común) o starters o plugin
    if data.contains("spring-boot-starter-parent")
        || data.contains("spring-boot-starter-")
        || data.contains("spring-boot-maven-plugin")
    {
        return Some("spring-boot");
    }
    // Quarkus / Micronaut → pack JVM (spring-boot) por ahora
    if data.contains("quarkus-") || data.contains("micronaut-") {
        return Some("spring-boot");
    }
    None // no deep match → que decida el fallback shallow
}

/// Detecta stack desde build.gradle[.kts]: Spring Boot Gradle plugin.
fn detect_gradle_stack(cwd: &Path) -> Option<&'static str> {
    for name in &["build.gradle", "build.gradle.kts"] {
        let path = cwd.join(name);
        if !path.exists() {
            continue;
        }
        let data = fs::read_to_string(path).ok()?;
        if data.contains("org.springframework.boot")
            || data.contains("spring-boot-gradle-plugin")
            || data.contains("spring-boot-starter-")
        {
            return Some("spring-boot");
        }
    }
    None
}

/// Detecta stack desde package.json: React, Next.js, Gatsby, etc.
fn detect_npm_stack(cwd: &Path) -> Option<&'static str> {
    let path = cwd.join("package.json");
    if !path.exists() {
        return None;
    }
    let data = fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&data).ok()?;

    // Recoge nombres de dependencias de todos los grupos típicos
    let mut deps: Vec<String> = Vec::new();
    for key in &["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(map) = json.get(key).and_then(|v| v.as_object()) {
            deps.extend(map.keys().map(|s| s.to_string()));
        }
    }

    // React / Next / Gatsby → react
    if deps.iter().any(|d| matches!(d.as_str(), "react" | "next" | "gatsby" | "react-native")) {
        return Some("react");
    }

    // Cualquier otro proyecto npm → node
    Some("node")
}

/// Detecta stack desde pyproject.toml / requirements.txt.
fn detect_python_stack(cwd: &Path) -> Option<&'static str> {
    if cwd.join("pyproject.toml").exists() || cwd.join("requirements.txt").exists() {
        return Some("python");
    }
    None
}

/// ¿El `package.json` declara `react` entre sus dependencias?
/// Usado por `detect_stack_shallow` como fallback rápido.
fn package_json_has_react(cwd: &Path) -> bool {
    let Ok(data) = fs::read_to_string(cwd.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) else {
        return false;
    };
    ["dependencies", "devDependencies"]
        .iter()
        .any(|key| json.get(*key).and_then(|deps| deps.get("react")).is_some())
}

/// El manifiesto de build (`pom.xml` o `build.gradle[.kts]`) acotado, para
/// fundamentar al modelo en las dependencias y versiones REALES del proyecto.
pub fn build_manifest(cwd: &Path) -> Option<(String, String)> {
    const MAX_LINES: usize = 160;
    for name in ["pom.xml", "build.gradle", "build.gradle.kts"] {
        let path = cwd.join(name);
        if !path.exists() {
            continue;
        }
        if let Ok(data) = fs::read_to_string(&path) {
            let total = data.lines().count();
            let content = if total > MAX_LINES {
                let head: String = data.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
                format!("{head}\n…[truncado: {total} líneas en total]")
            } else {
                data
            };
            return Some((name.to_string(), content));
        }
    }
    None
}

/// ¿La ruta es código fuente compilable o el manifiesto de build?
fn is_build_source(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".java")
        || p.ends_with(".kt")
        || p.ends_with(".rs")
        || p.ends_with("pom.xml")
        || p.ends_with("build.gradle")
        || p.ends_with("build.gradle.kts")
        || p.ends_with("cargo.toml")
}

/// ¿Alguna de las escrituras toca código fuente o el build (para disparar la
/// verificación automática de compilación)?
pub fn touches_build(writes: &[FileWrite]) -> bool {
    writes.iter().any(|w| is_build_source(&w.path))
}

/// Igual que [`touches_build`] pero para ediciones quirúrgicas.
pub fn edits_touch_build(edits: &[FileEdit]) -> bool {
    edits.iter().any(|e| is_build_source(&e.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_stack_reconoce_el_repo_de_dpx() {
        let dir = std::env::temp_dir().join(format!("dpx-selfdetect-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        // Crate Rust genérico → pack "rust".
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"otra-cosa\"\n").unwrap();
        assert_eq!(detect_stack(&dir), Some("rust"));
        assert!(!is_dpx_repo(&dir));

        // El propio dpx (name = "dpx-cli") → pack "dpx" (auto-edición).
        fs::write(dir.join("Cargo.toml"), "[package]\nname = \"dpx-cli\"\nversion = \"0.2.0\"\n")
            .unwrap();
        assert!(is_dpx_repo(&dir));
        assert_eq!(detect_stack(&dir), Some("dpx"));

        fs::remove_dir_all(&dir).unwrap();
    }
}
