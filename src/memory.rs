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

// Fase 2 en construcción: el motor está validado a nivel de compilación/enlace,
// pero el store/ingesta/recuperación aún no se cablean a la app. Se quita este
// allow al conectar la memoria al loop.
#![allow(dead_code)]

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

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
    /// Carga el modelo local (lo descarga la primera vez). Falla con un mensaje
    /// claro si el runtime de ONNX no arranca en esta máquina (p. ej. la
    /// `onnxruntime.dll` en Windows) — el llamador decide si degradar.
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(TextInitOptions::new(MODEL))
            .context("no pude inicializar el motor de embeddings local (fastembed/ONNX)")?;
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
