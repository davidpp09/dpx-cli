//! Sandbox y verificación de los casos de ESCRITURA.
//!
//! Un caso de escritura no se puede medir sobre tu repo: el modelo tiene que
//! poder editar de verdad para que la prueba signifique algo. Cada corrida
//! recibe su propia copia desechable de un proyecto mínimo, sembrada desde el
//! fixture del caso, y el agente trabaja con esa carpeta como raíz — `safe_target`
//! ya impide salir de ella con `..` o rutas absolutas.
//!
//! Lo que se puntúa NO es lo que el modelo dice haber hecho, sino lo que quedó
//! escrito en disco.

use std::path::Path;

use anyhow::{Context, Result};

/// Un archivo semilla: `(ruta relativa, contenido)`.
pub type Seed = (&'static str, &'static str);

/// Aserción sobre el estado FINAL de los archivos tras el caso.
#[derive(Debug, Clone, Copy)]
pub enum Check {
    /// El archivo existe y contiene TODOS estos fragmentos.
    Contains(&'static str, &'static [&'static str]),
    /// El archivo NO contiene NINGUNO de estos fragmentos. Es el que atrapa los
    /// renames a medias y el código viejo que se quedó colgando.
    Absent(&'static str, &'static [&'static str]),
}

impl Check {
    /// `None` si la aserción se cumple; si no, la explicación del fallo.
    pub fn failure(&self, root: &Path) -> Option<String> {
        let (file, _) = match self {
            Check::Contains(f, n) | Check::Absent(f, n) => (f, n),
        };
        let Ok(body) = std::fs::read_to_string(root.join(file)) else {
            return Some(format!("{file}: no existe"));
        };
        match self {
            Check::Contains(_, needles) => {
                let faltan: Vec<&str> =
                    needles.iter().copied().filter(|n| !body.contains(n)).collect();
                (!faltan.is_empty()).then(|| format!("{file}: falta `{}`", faltan.join("`, `")))
            }
            Check::Absent(_, needles) => {
                let sobran: Vec<&str> =
                    needles.iter().copied().filter(|n| body.contains(n)).collect();
                (!sobran.is_empty())
                    .then(|| format!("{file}: quedó `{}`", sobran.join("`, `")))
            }
        }
    }
}

/// Siembra el proyecto de partida en `root`, creando subdirectorios.
///
/// Genérico sobre el tipo de las cadenas para aceptar tanto los fixtures
/// literales (`&[(&str, &str)]`) como los GENERADOS en tiempo de ejecución
/// (`Vec<(String, String)>`): un archivo de 2800 líneas no cabe como literal
/// en el fuente, pero hace falta para probar qué pasa cuando `read_file` trunca.
pub fn seed<P: AsRef<str>, C: AsRef<str>>(root: &Path, files: &[(P, C)]) -> Result<()> {
    for (rel, body) in files {
        let target = root.join(rel.as_ref());
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("no pude crear {}", parent.display()))?;
        }
        std::fs::write(&target, body.as_ref())
            .with_context(|| format!("no pude sembrar {}", target.display()))?;
    }
    Ok(())
}

/// Todas las aserciones que fallaron. Vacío = el caso quedó bien resuelto.
pub fn failures(root: &Path, checks: &[Check]) -> Vec<String> {
    checks.iter().filter_map(|c| c.failure(root)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(nombre: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("dpx-sandbox-{}-{nombre}-{}", std::process::id(), line!()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn seed_crea_subdirectorios_y_contenido() {
        let dir = tmp("seed");
        seed(&dir, &[("src/anidado/a.rs", "hola")]).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("src/anidado/a.rs")).unwrap(), "hola");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn contains_y_absent_detectan_el_rename_a_medias() {
        let dir = tmp("checks");
        seed(&dir, &[("src/a.rs", "fn total_con_iva() {}\nfn otra() { calc_total(); }")]).unwrap();

        // El nombre nuevo está…
        assert!(Check::Contains("src/a.rs", &["total_con_iva"]).failure(&dir).is_none());
        // …pero un uso del viejo se quedó: eso es exactamente el fallo a cazar.
        let f = Check::Absent("src/a.rs", &["calc_total"]).failure(&dir).unwrap();
        assert!(f.contains("quedó"), "{f}");

        // Fragmento ausente en un Contains.
        let f = Check::Contains("src/a.rs", &["no_existe"]).failure(&dir).unwrap();
        assert!(f.contains("falta"), "{f}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn archivo_inexistente_falla_en_ambos_sentidos() {
        let dir = tmp("vacio");
        // Ni un Contains ni un Absent pueden darse por buenos si el archivo no
        // está: un modelo que borró el archivo no debe pasar el Absent.
        assert!(Check::Contains("nada.rs", &["x"]).failure(&dir).unwrap().contains("no existe"));
        assert!(Check::Absent("nada.rs", &["x"]).failure(&dir).unwrap().contains("no existe"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn failures_junta_todo_lo_que_falla() {
        let dir = tmp("multi");
        seed(&dir, &[("a.rs", "viejo")]).unwrap();
        let checks = [
            Check::Contains("a.rs", &["viejo"]), // pasa
            Check::Absent("a.rs", &["viejo"]),   // falla
            Check::Contains("b.rs", &["algo"]),  // falla: no existe
        ];
        assert_eq!(failures(&dir, &checks).len(), 2);
        assert!(failures(&dir, &checks[..1]).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
