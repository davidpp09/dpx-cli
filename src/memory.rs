//! Memoria semántica de largo plazo (Fase 2) — el "acabar con el problema del
//! contexto" de dpx.
//!
//! Idea: en vez de meter TODO en la ventana del modelo (imposible), guardamos
//! fragmentos fuera (`.dpx/memory.jsonl`), cada uno con su **embedding** (un
//! vector que captura su significado), y en cada turno recuperamos solo los más
//! parecidos a lo que el usuario pregunta (similitud coseno). Los embeddings son
//! **locales** (fastembed / ONNX, modelo BGE-small): gratis, privados, offline.
//!
//! Este módulo arranca por lo mínimo VALIDABLE: generar un embedding real en la
//! máquina y la matemática de similitud. El store + la ingesta + la recuperación
//! se construyen encima una vez verificado que el motor corre aquí.

// Algunos helpers (DIM, embed múltiple) se usan solo en tests/futuras fases de
// la memoria; se conservan como API del módulo.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use serde::{Deserialize, Serialize};

/// Modelo de embeddings: BGE-small (384 dimensiones). Pequeño y rápido en CPU,
/// más que suficiente para recuperar memoria personal. Se descarga una vez a la
/// caché de fastembed y luego corre offline.
const MODEL: EmbeddingModel = EmbeddingModel::BGESmallENV15;

/// Dimensión del vector que produce [`MODEL`].
pub const DIM: usize = 384;

/// Motor de embeddings local. Cargar el modelo es caro (lee pesos a memoria), así
/// que se crea UNA vez y se reutiliza para todas las consultas de la sesión.
pub struct Embedder {
    model: TextEmbedding,
}

impl Embedder {
    /// Carga el modelo local. La primera vez lo descarga (~130 MB desde
    /// HuggingFace); después corre offline.
    ///
    /// Detalle clave en Windows: tras una descarga previa, hf-hub intenta
    /// REVALIDAR el modelo contra HuggingFace al arrancar y, si la red lo bloquea
    /// (error 10054), aborta SIN usar la caché que ya tiene. Por eso, si el
    /// intento normal falla, reintentamos en **modo offline** (`HF_HUB_OFFLINE`),
    /// que ignora la red y usa solo los archivos locales. Resultado: solo se
    /// necesita red la PRIMera vez; luego dpx arranca aunque no haya internet.
    pub fn new() -> Result<Self> {
        // 1) Intento normal: descarga el modelo si aún no está en caché.
        if let Ok(model) = TextEmbedding::try_new(TextInitOptions::new(MODEL)) {
            return Ok(Self { model });
        }
        // 2) Falló (típicamente la revalidación de red sobre un modelo ya
        //    cacheado). Forzamos offline y reintentamos contra la caché local.
        //    SAFETY: arranque del motor; aceptamos esta escritura de entorno como
        //    best-effort (hf-hub la lee a continuación en esta misma llamada).
        unsafe {
            std::env::set_var("HF_HUB_OFFLINE", "1");
        }
        let model = TextEmbedding::try_new(TextInitOptions::new(MODEL)).context(
            "no pude inicializar el motor de embeddings local. La primera vez dpx descarga el \
             modelo (~130 MB) desde HuggingFace; si tu red lo bloquea, descárgalo una vez en otra \
             red y dpx lo reusará desde su caché (.fastembed_cache).",
        )?;
        Ok(Self { model })
    }

    /// Embebe varios textos de una vez (más eficiente que de a uno).
    pub fn embed(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.model
            .embed(texts, None)
            .context("falló la generación de embeddings")
    }

    /// Embebe un solo texto.
    pub fn embed_one(&mut self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(&[text])?;
        v.pop().context("el motor no devolvió ningún embedding")
    }
}

/// Similitud coseno entre dos vectores: 1.0 = idénticos en dirección, 0.0 =
/// ortogonales. Es la "matemática" que decide qué recordar. Devuelve 0.0 si
/// algún vector es nulo (evita dividir por cero).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// Una entrada de memoria de largo plazo: el texto, su embedding y metadatos.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MemoryEntry {
    /// El contenido recordable (una nota, un resumen de sesión, una decisión).
    pub text: String,
    /// El embedding del texto (para buscar por similitud).
    pub vector: Vec<f32>,
    /// Tipo: "nota" (del usuario), "resumen" (cierre de sesión), etc.
    #[serde(default)]
    pub kind: String,
    /// Fecha de creación (YYYY-MM-DD).
    #[serde(default)]
    pub created: String,
}

/// Almacén de memoria semántica persistido en `.dpx/memory.jsonl` (una entrada
/// JSON por línea — append-only, robusto y fácil de inspeccionar). Carga todo en
/// memoria al abrir; la búsqueda es lineal por coseno (suficiente para decenas
/// de miles de entradas en una herramienta personal).
pub struct MemoryStore {
    path: PathBuf,
    entries: Vec<MemoryEntry>,
}

impl MemoryStore {
    /// Abre (o crea vacío) el store en `<dpx_dir>/memory.jsonl`. Tolera líneas
    /// corruptas: las salta en vez de fallar.
    pub fn load(dpx_dir: &Path) -> Self {
        let path = dpx_dir.join("memory.jsonl");
        let entries = match std::fs::read_to_string(&path) {
            Ok(data) => data
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str::<MemoryEntry>(l).ok())
                .collect(),
            Err(_) => Vec::new(),
        };
        Self { path, entries }
    }

    /// ¿Hay algo guardado? (Si está vacío, el CLI ni carga el modelo.)
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Número de recuerdos.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Añade una entrada y la persiste (append a la línea JSONL, sin reescribir
    /// todo el archivo).
    pub fn add(&mut self, entry: MemoryEntry) -> Result<()> {
        use std::io::Write;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let line = serde_json::to_string(&entry).context("no pude serializar el recuerdo")?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("no pude abrir {}", self.path.display()))?;
        writeln!(file, "{line}").context("no pude escribir el recuerdo")?;
        self.entries.push(entry);
        Ok(())
    }

    /// Devuelve los `k` recuerdos más parecidos a `query_vec`, de mayor a menor
    /// similitud, descartando los que no llegan a `min_score` (ruido).
    pub fn search(&self, query_vec: &[f32], k: usize, min_score: f32) -> Vec<(f32, &MemoryEntry)> {
        let mut scored: Vec<(f32, &MemoryEntry)> = self
            .entries
            .iter()
            .map(|e| (cosine(query_vec, &e.vector), e))
            .filter(|(s, _)| *s >= min_score)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(k);
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_mide_similitud() {
        let a = [1.0, 0.0, 0.0];
        let b = [1.0, 0.0, 0.0];
        let c = [0.0, 1.0, 0.0];
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-6, "iguales → 1.0");
        assert!(cosine(&a, &c).abs() < 1e-6, "ortogonales → 0.0");
        // Vector nulo no rompe.
        assert_eq!(cosine(&a, &[0.0, 0.0, 0.0]), 0.0);
    }

    fn entry(text: &str, v: Vec<f32>) -> MemoryEntry {
        MemoryEntry { text: text.into(), vector: v, kind: "nota".into(), created: "2026-06-14".into() }
    }

    #[test]
    fn store_persiste_y_recupera_por_similitud() {
        let dir = std::env::temp_dir().join(format!("dpx-mem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let mut store = MemoryStore::load(&dir);
        assert!(store.is_empty());
        store.add(entry("uso JPA con repositorios", vec![1.0, 0.0, 0.0])).unwrap();
        store.add(entry("el front es React", vec![0.0, 1.0, 0.0])).unwrap();
        store.add(entry("repositorios y entidades", vec![0.9, 0.1, 0.0])).unwrap();
        assert_eq!(store.len(), 3);

        // Buscar algo cercano al primer vector → trae los dos de JPA, no el de React.
        let hits = store.search(&[1.0, 0.0, 0.0], 2, 0.1);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].1.text.contains("JPA") || hits[0].1.text.contains("repositorios"));
        assert!(hits.iter().all(|(_, e)| !e.text.contains("React")), "no debe traer lo irrelevante");

        // Reabrir desde disco: la persistencia JSONL funciona.
        let reopened = MemoryStore::load(&dir);
        assert_eq!(reopened.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_respeta_min_score_y_k() {
        let store = {
            let dir = std::env::temp_dir().join(format!("dpx-mem2-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let mut s = MemoryStore::load(&dir);
            s.add(entry("a", vec![1.0, 0.0])).unwrap();
            s.add(entry("b", vec![0.0, 1.0])).unwrap(); // ortogonal → score 0
            std::fs::remove_dir_all(&dir).ok();
            s
        };
        // min_score alto descarta el ortogonal.
        let hits = store.search(&[1.0, 0.0], 5, 0.5);
        assert_eq!(hits.len(), 1, "el ortogonal (score 0) se filtra");
    }

    /// Validación EN VIVO del motor local: descarga el modelo y comprueba que
    /// produce vectores reales en ESTA máquina, y que la semántica funciona
    /// (dos frases parecidas se parecen más que una ajena). Ignorado por defecto
    /// (descarga ~130 MB la 1ª vez). Correr: `cargo test embeddings_reales -- --ignored`.
    #[test]
    #[ignore = "descarga el modelo de embeddings (~130MB); valida el motor local"]
    fn embeddings_reales_funcionan_en_esta_maquina() {
        let mut emb = Embedder::new().expect("el motor debe arrancar");
        let vs = emb
            .embed(&[
                "Spring Boot inyección de dependencias",
                "inversión de control en Spring",
                "receta de tacos al pastor",
            ])
            .expect("debe embeber");
        assert_eq!(vs.len(), 3);
        assert_eq!(vs[0].len(), DIM, "dimensión esperada del modelo");

        let sim_relacionadas = cosine(&vs[0], &vs[1]);
        let sim_ajena = cosine(&vs[0], &vs[2]);
        assert!(
            sim_relacionadas > sim_ajena,
            "dos frases del mismo tema deben parecerse más ({sim_relacionadas}) que una ajena ({sim_ajena})"
        );
    }
}
