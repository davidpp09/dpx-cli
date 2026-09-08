//! Los casos del banco de pruebas y el preamble que comparten los arms.
//!
//! Todos los casos son de SOLO LECTURA y sobre ESTE repo: así el banco no
//! depende de ningún proyecto externo y las respuestas se pueden verificar
//! contra el código que tienes delante.

use std::path::Path;

use super::sandbox::{Check, Seed};

/// Genera archivos del sandbox en tiempo de ejecución, para los que no caben
/// como literal en el fuente.
type Generador = fn() -> Vec<(String, String)>;

/// Un caso del banco.
///
/// Hay dos familias. Los de **lectura** corren sobre este mismo repo y se
/// puntúan por lo que el modelo RESPONDE (`expect`). Los de **escritura** corren
/// sobre un sandbox desechable sembrado con `fixture`, y se puntúan por lo que
/// el modelo DEJA EN DISCO (`checks`) — que es lo único que cuenta cuando el
/// trabajo es editar código.
pub struct Case {
    /// Nombre corto, para filtrar con `--case` y nombrar el transcript.
    pub name: &'static str,
    /// Lo que se le pide al modelo. Idéntico para todos los arms.
    pub prompt: &'static str,
    /// Criterio sobre la RESPUESTA: TODOS los grupos deben aparecer; basta con
    /// que aparezca UNA de las alternativas de cada grupo (el modelo puede
    /// decir "3/4" o "75%" y las dos están bien).
    pub expect: &'static [&'static [&'static str]],
    /// Proyecto de partida del sandbox. Vacío = caso de lectura sobre el repo.
    pub fixture: &'static [Seed],
    /// Archivos del sandbox generados en tiempo de ejecución, para los que no
    /// caben como literal. Se siembran DESPUÉS de `fixture`.
    pub generado: Option<Generador>,
    /// Criterio sobre los ARCHIVOS resultantes (solo casos de escritura).
    pub checks: &'static [Check],
}

impl Case {
    /// ¿Este caso edita archivos? Determina el sandbox y las herramientas.
    pub fn is_write(&self) -> bool {
        !self.fixture.is_empty() || self.generado.is_some()
    }

    /// Siembra el sandbox de este caso: primero los literales, luego lo generado.
    pub fn seed(&self, root: &Path) -> anyhow::Result<()> {
        super::sandbox::seed(root, self.fixture)?;
        if let Some(generador) = self.generado {
            super::sandbox::seed(root, &generador())?;
        }
        Ok(())
    }

    /// Grupos esperados que NO aparecieron en la respuesta. Vacío = acierto.
    /// La comparación ignora mayúsculas para no castigar el formato.
    pub fn missing(&self, answer: &str) -> Vec<String> {
        let hay = answer.to_lowercase();
        self.expect
            .iter()
            .filter(|group| !group.iter().any(|alt| hay.contains(&alt.to_lowercase())))
            .map(|group| group.join(" | "))
            .collect()
    }

    /// Todo lo que falló: lo que faltó en la respuesta MÁS lo que quedó mal en
    /// disco. `root` es el sandbox (irrelevante en los casos de lectura).
    pub fn failures(&self, answer: &str, root: &Path) -> Vec<String> {
        let mut fallos = self.missing(answer);
        fallos.extend(super::sandbox::failures(root, self.checks));
        fallos
    }

    /// `false` si el caso no tiene criterio automático (juicio humano).
    pub fn scored(&self) -> bool {
        !self.expect.is_empty() || !self.checks.is_empty()
    }
}

/// Atajo para declarar un caso de LECTURA sobre este repo.
const fn leer(
    name: &'static str,
    prompt: &'static str,
    expect: &'static [&'static [&'static str]],
) -> Case {
    Case { name, prompt, expect, fixture: &[], generado: None, checks: &[] }
}

/// Atajo para declarar un caso de ESCRITURA con fixture literal.
const fn escribir(
    name: &'static str,
    prompt: &'static str,
    fixture: &'static [Seed],
    checks: &'static [Check],
) -> Case {
    Case { name, prompt, expect: &[], fixture, generado: None, checks }
}

/// Genera un archivo MÁS GRANDE de lo que `read_file` devuelve de una vez
/// (trunca a 2500 líneas) con el objetivo al final. Obliga al agente a
/// localizarlo —buscando o paginando— y, sobre todo, a editarlo de forma
/// quirúrgica: quien intente reescribir el archivo entero desde memoria lo
/// destruye, y los checks de las reglas lejanas lo delatan.
fn reglas_grandes() -> Vec<(String, String)> {
    let mut body = String::from("//! Reglas de negocio (generado).\n\n");
    for n in 1..=2800 {
        body.push_str(&format!("pub fn regla_{n:04}(x: i64) -> i64 {{ x + {n} }}\n"));
    }
    body.push_str("\npub const LIMITE_MAXIMO: i64 = 999;\n");
    vec![("src/reglas.rs".to_string(), body)]
}

/// La suite. Casos de lectura sobre este repo + casos de escritura sobre un
/// sandbox. Los de escritura son los que de verdad deciden el default del
/// router: `code` y `hack` se ganan el sueldo editando código, no explicándolo.
pub const CASES: &[Case] = &[
    leer(
        "rig-version",
        "¿Qué versión de rig-core usa este proyecto? Responde solo con el número.",
        &[&["0.37"]],
    ),
    leer(
        "context-budget",
        "¿Cuánto vale la constante CONTEXT_BUDGET y en qué archivo está definida?",
        &[&["400"], &["router.rs"]],
    ),
    leer(
        "tarifa-pro",
        "Según el código de este proyecto, ¿cuántos USD por 1M de tokens de salida cuesta el \
         tier pro de DeepSeek?",
        &[&["0.87"]],
    ),
    leer(
        "umbral-compact",
        "¿A partir de qué fracción del presupuesto de contexto dpx compacta el historial \
         automáticamente, y qué función lo calcula?",
        &[&["compact_threshold"], &["75", "3/4", "0.75"]],
    ),
    // Multi-hop: obliga a cruzar la definición de las tools con el ejecutor que
    // las atiende. Es el caso de lectura que más se acerca a trabajo agéntico.
    leer(
        "tools-subagente",
        "¿Qué herramientas puede EJECUTAR realmente un subagente de dpx (no las que se le \
         anuncian, las que el ejecutor atiende) y en qué función se filtran?",
        &[
            &["read_file"],
            &["search_project"],
            &["web_search"],
            &["subagent_tool", "definitions_read_only", "read_only"],
        ],
    ),
    // Sin criterio automático: aquí se compara la CALIDAD de la explicación.
    leer(
        "delegacion",
        "Explica en máximo 6 líneas cómo dpx decide si delega una petición del usuario a un \
         subagente, y en qué casos NO delega.",
        &[],
    ),
    // ── Casos de ESCRITURA (sandbox) ────────────────────────────────
    // El clásico: un cambio de una línea. Lo que se mide no es acertar el
    // valor —eso es trivial— sino NO llevarse por delante el resto del archivo
    // al reescribirlo, que es el fallo más caro de un agente que edita.
    escribir(
        "edit-puntual",
        "Cambia el puerto por defecto de 8080 a 9090 en src/config.rs. No toques nada más del \
         archivo.",
        &[(
            "src/config.rs",
            "//! Configuración del servidor.\n\n\
             pub const PUERTO: u16 = 8080;\n\
             pub const MAX_CONEXIONES: usize = 100;\n\
             pub const NOMBRE: &str = \"servidor-demo\";\n\n\
             pub fn describe() -> String {\n    \
                 format!(\"{NOMBRE} en el puerto {PUERTO}\")\n\
             }\n",
        )],
        &[
            Check::Contains("src/config.rs", &["9090"]),
            Check::Absent("src/config.rs", &["8080"]),
            // Lo de verdad importante: el resto del archivo sigue ahí.
            Check::Contains(
                "src/config.rs",
                &["MAX_CONEXIONES", "servidor-demo", "pub fn describe"],
            ),
        ],
    ),
    // Añadir código sin romper el que ya estaba.
    escribir(
        "agregar-tests",
        "Agrega tests unitarios para `suma` y `resta` en src/calc.rs, en un módulo `tests` al \
         final del archivo.",
        &[(
            "src/calc.rs",
            "//! Utilidades de cálculo.\n\n\
             pub fn suma(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n\
             pub fn resta(a: i32, b: i32) -> i32 {\n    a - b\n}\n",
        )],
        &[
            Check::Contains(
                "src/calc.rs",
                &["#[cfg(test)]", "mod tests", "#[test]", "suma(", "resta("],
            ),
            // No borró el código original para meter los tests.
            Check::Contains("src/calc.rs", &["pub fn suma", "pub fn resta"]),
        ],
    ),
    // Un rename que obliga a BUSCAR los usos antes de editar. Un agente que
    // solo mira el archivo obvio deja el proyecto roto, y los `Absent` de los
    // otros dos archivos lo cazan.
    escribir(
        "rename-multiarchivo",
        "Renombra la función `calc_total` a `total_con_iva` en TODO el proyecto, incluyendo \
         todos sus usos.",
        &[
            (
                "src/precio.rs",
                "//! Cálculo de precios.\n\n\
                 pub fn calc_total(base: f64) -> f64 {\n    base * 1.16\n}\n",
            ),
            (
                "src/main.rs",
                "mod precio;\nmod reporte;\n\n\
                 fn main() {\n    \
                     let t = precio::calc_total(100.0);\n    \
                     println!(\"total: {t}\");\n\
                 }\n",
            ),
            (
                "src/reporte.rs",
                "use crate::precio::calc_total;\n\n\
                 pub fn linea(base: f64) -> String {\n    \
                     format!(\"{base} -> {}\", calc_total(base))\n\
                 }\n",
            ),
        ],
        &[
            Check::Contains("src/precio.rs", &["total_con_iva"]),
            Check::Absent("src/precio.rs", &["calc_total"]),
            Check::Contains("src/main.rs", &["total_con_iva"]),
            Check::Absent("src/main.rs", &["calc_total"]),
            Check::Contains("src/reporte.rs", &["total_con_iva"]),
            Check::Absent("src/reporte.rs", &["calc_total"]),
        ],
    ),
    // ── Casos DUROS ─────────────────────────────────────────────────
    // Sin ruta: el prompt dice QUÉ, no DÓNDE. Hay que buscarlo entre varios
    // módulos. El señuelo (`POOL_TIMEOUT`) castiga al que cambia el primer
    // timeout que se encuentra en vez del que se le pidió.
    escribir(
        "edit-sin-ruta",
        "El cliente HTTP se está quedando corto: sube su timeout a 60 segundos.",
        &[
            (
                "src/main.rs",
                "mod infra;\n\nfn main() {\n    println!(\"arranque\");\n}\n",
            ),
            (
                "src/infra/mod.rs",
                "pub mod cliente;\npub mod db;\n",
            ),
            (
                "src/infra/cliente.rs",
                "//! Cliente HTTP contra la API externa.\n\n\
                 pub const TIMEOUT_SEGUNDOS: u64 = 30;\n\n\
                 pub fn pedir(url: &str) -> String {\n    \
                     format!(\"GET {url} (timeout {TIMEOUT_SEGUNDOS}s)\")\n\
                 }\n",
            ),
            (
                "src/infra/db.rs",
                "//! Pool de conexiones a la base de datos.\n\n\
                 pub const POOL_TIMEOUT: u64 = 5;\n",
            ),
        ],
        &[
            Check::Contains("src/infra/cliente.rs", &["TIMEOUT_SEGUNDOS: u64 = 60"]),
            Check::Absent("src/infra/cliente.rs", &["= 30"]),
            // El señuelo NO se toca: se pidió el del cliente HTTP, no el del pool.
            Check::Contains("src/infra/db.rs", &["POOL_TIMEOUT: u64 = 5"]),
        ],
    ),
    // La trampa: un reemplazo literal de `calc_total` convierte
    // `calc_total_neto` en `total_con_iva_neto` y rompe la otra función. Es el
    // fallo clásico de renombrar con búsqueda de texto en vez de por símbolo.
    escribir(
        "rename-con-trampa",
        "Renombra SOLO la función `calc_total` a `total_con_iva`, en todo el proyecto. \
         `calc_total_neto` es OTRA función y debe quedar intacta.",
        &[
            (
                "src/precio.rs",
                "//! Cálculo de precios.\n\n\
                 pub fn calc_total(base: f64) -> f64 {\n    base * 1.16\n}\n\n\
                 pub fn calc_total_neto(base: f64) -> f64 {\n    base\n}\n",
            ),
            (
                "src/uso.rs",
                "use crate::precio::{calc_total, calc_total_neto};\n\n\
                 pub fn con_iva(b: f64) -> f64 {\n    calc_total(b)\n}\n\n\
                 pub fn sin_iva(b: f64) -> f64 {\n    calc_total_neto(b)\n}\n",
            ),
        ],
        &[
            Check::Contains("src/precio.rs", &["total_con_iva", "calc_total_neto"]),
            // La firma del desastre: el reemplazo ciego deja este nombre.
            Check::Absent("src/precio.rs", &["total_con_iva_neto"]),
            Check::Contains("src/uso.rs", &["total_con_iva", "calc_total_neto"]),
            Check::Absent("src/uso.rs", &["total_con_iva_neto"]),
        ],
    ),
    // Archivo más grande de lo que `read_file` devuelve de una vez. Quien
    // reescriba el archivo entero desde memoria pierde 2800 funciones, y los
    // checks de las reglas lejanas lo delatan.
    Case {
        name: "archivo-grande",
        prompt: "En src/reglas.rs, cambia el valor de la constante LIMITE_MAXIMO a 4242.",
        expect: &[],
        fixture: &[],
        generado: Some(reglas_grandes),
        checks: &[
            Check::Contains("src/reglas.rs", &["LIMITE_MAXIMO: i64 = 4242"]),
            Check::Absent("src/reglas.rs", &["= 999;"]),
            // El archivo sigue COMPLETO: principio, medio y final.
            Check::Contains("src/reglas.rs", &["regla_0001", "regla_1400", "regla_2800"]),
        ],
    },
];

/// Preamble de los arms. Deliberadamente NEUTRO (sin focus pack): lo que se
/// compara es el modelo y su esfuerzo de razonamiento, no el prompt. Incluye el
/// árbol del proyecto igual que hace el subagente real, para que ninguna
/// configuración arranque a ciegas.
///
/// `write` cambia el encuadre pero NO da pistas de cómo resolver: si el
/// preamble dijera "busca los usos antes de renombrar", el caso duro dejaría de
/// medir nada.
pub fn preamble(root: &Path, write: bool) -> String {
    let mut p = String::from(if write {
        "Eres un agente de programación trabajando sobre un proyecto Rust. Tienes herramientas \
         para leer (read_file, search_project) y para MODIFICAR archivos (write_file, \
         edit_file).\n\n\
         REGLAS:\n\
         - Aplica los cambios TÚ con las herramientas; no describas el cambio ni le pidas al \
           usuario que lo haga.\n\
         - Investiga antes de editar; no adivines el contenido de un archivo.\n\
         - Cuando el trabajo esté hecho, dilo en TEXTO PLANO y para de pedir herramientas.\n\n\
         # Árbol del proyecto\n```\n"
    } else {
        "Eres un agente de análisis de código trabajando sobre un proyecto Rust. Tienes \
         herramientas de SOLO LECTURA: read_file, search_project y web_search.\n\n\
         REGLAS:\n\
         - Investiga con las herramientas ANTES de responder; no adivines desde el nombre de un \
           archivo.\n\
         - Cuando tengas la respuesta, dala en TEXTO PLANO y para de pedir herramientas.\n\
         - Sé directo y concreto: datos, rutas y números. Nada de relleno.\n\n\
         # Árbol del proyecto\n```\n"
    });
    p.push_str(&crate::fs::project_tree(root));
    p.push_str("```\n");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_acepta_alternativas_e_ignora_mayusculas() {
        let c = leer("t", "p", &[&["compact_threshold"], &["75", "3/4"]]);
        // Una alternativa de cada grupo basta.
        assert!(c.missing("La función COMPACT_THRESHOLD corta a 3/4 del budget").is_empty());
        assert!(c.missing("compact_threshold() = 75% del presupuesto").is_empty());
        // Falta el grupo del umbral.
        assert_eq!(c.missing("lo hace compact_threshold").len(), 1);
    }

    #[test]
    fn caso_sin_criterio_no_se_puntua() {
        let c = leer("t", "p", &[]);
        assert!(!c.scored());
        assert!(!c.is_write());
        assert!(c.missing("cualquier cosa").is_empty());
    }

    #[test]
    fn un_caso_de_escritura_se_puntua_por_el_disco() {
        let dir = std::env::temp_dir().join(format!("dpx-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let c = escribir(
            "w",
            "p",
            &[("a.rs", "const X: u16 = 8080;")],
            &[Check::Contains("a.rs", &["9090"]), Check::Absent("a.rs", &["8080"])],
        );
        assert!(c.is_write() && c.scored());

        // El modelo AFIRMA haberlo hecho pero el disco sigue con el valor viejo:
        // debe fallar igual. Eso es justo lo que un caso de escritura añade.
        c.seed(&dir).unwrap();
        assert_eq!(c.failures("Listo, ya cambié el puerto a 9090.", &dir).len(), 2);

        std::fs::write(dir.join("a.rs"), "const X: u16 = 9090;").unwrap();
        assert!(c.failures("hecho", &dir).is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn la_suite_esta_bien_formada() {
        assert!(CASES.len() >= 5);
        assert!(CASES.iter().any(|c| c.is_write()), "la suite necesita casos de escritura");
        for c in CASES {
            assert!(!c.name.is_empty() && !c.prompt.is_empty(), "caso vacío: {}", c.name);
            // Nombres únicos: se usan como nombre de archivo del transcript.
            assert_eq!(
                CASES.iter().filter(|o| o.name == c.name).count(),
                1,
                "nombre de caso duplicado: {}",
                c.name
            );
            for group in c.expect {
                assert!(!group.is_empty(), "grupo de expect vacío en {}", c.name);
            }
            // Un caso de escritura sin checks no mediría nada.
            assert_eq!(
                c.is_write(),
                !c.checks.is_empty(),
                "fixture y checks van juntos: {}",
                c.name
            );
        }
    }

    /// Los fixtures son el punto de partida: si ya cumplieran los checks, el
    /// caso pasaría sin que el modelo tocara nada.
    #[test]
    fn ningun_fixture_cumple_ya_sus_propios_checks() {
        for c in CASES.iter().filter(|c| c.is_write()) {
            let dir = std::env::temp_dir()
                .join(format!("dpx-fixture-{}-{}", std::process::id(), c.name));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            c.seed(&dir).unwrap();
            // Sembrar de verdad importa: si el sandbox quedara vacío, los checks
            // fallarían por "no existe" y el test pasaría sin comprobar nada.
            for check in c.checks {
                let (archivo, _) = match check {
                    Check::Contains(f, n) | Check::Absent(f, n) => (f, n),
                };
                assert!(
                    dir.join(archivo).exists(),
                    "`{}` chequea `{archivo}`, que su fixture no siembra",
                    c.name
                );
            }
            assert!(
                !c.failures("", &dir).is_empty(),
                "el fixture de `{}` ya pasa los checks sin hacer nada",
                c.name
            );
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
