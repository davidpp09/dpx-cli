//! Checkpoints de archivos por turno, para `/undo`.
//!
//! Antes de que dpx modifique un archivo, guardamos su contenido ANTERIOR (bytes
//! crudos, binario-seguro) en un snapshot. Todos los cambios de UN turno forman
//! un grupo; `/undo` restaura el último grupo: reescribe lo que existía y borra
//! lo que dpx creó nuevo. Es en memoria y por sesión, y **no toca git** — así
//! jamás clobberea cambios tuyos ni el estado del repo; solo revierte lo que el
//! propio dpx escribió en este turno.
//!
//! La lógica vive en métodos de [`State`] (testeables en una instancia local,
//! sin estado global). Las funciones públicas operan sobre un único `State`
//! global (patrón single-user, como `ui::CANCEL`); fuera de un turno activo
//! ([`TurnGuard`]), capturar es un no-op.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// El contenido anterior de un archivo tocado en el turno. `before == None`
/// significa que el archivo NO existía (deshacer = borrarlo).
struct FileSnapshot {
    path: PathBuf,
    before: Option<Vec<u8>>,
}

/// Un archivo que dpx cambió en la sesión, para `/diff`: contenido base (al
/// inicio) vs. actual. `None` = inexistente (nuevo, o borrado).
pub struct FileChange {
    pub path: PathBuf,
    pub before: Option<String>,
    pub now: Option<String>,
}

struct State {
    /// ¿Hay un turno en curso? Fuera de un turno no se captura nada.
    active: bool,
    /// Snapshots del turno en curso (uno por archivo, el PRIMER estado visto).
    pending: Vec<FileSnapshot>,
    /// Pila de grupos commiteados; cada `undo` saca el de arriba.
    stack: Vec<Vec<FileSnapshot>>,
    /// Estado de cada archivo al verlo por PRIMERA vez en la sesión (la línea
    /// base contra la que `/diff` compara). Sobrevive a los `undo`.
    baseline: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl State {
    const fn new() -> Self {
        State {
            active: false,
            pending: Vec::new(),
            stack: Vec::new(),
            baseline: Vec::new(),
        }
    }

    fn begin(&mut self) {
        self.active = true;
        self.pending.clear();
    }

    fn commit(&mut self) {
        self.active = false;
        if !self.pending.is_empty() {
            let group = std::mem::take(&mut self.pending);
            self.stack.push(group);
        }
    }

    /// Captura el estado ANTERIOR de `target` (absoluto) antes de modificarlo.
    /// No-op si no hay turno activo o si ya se capturó este turno. Si el archivo
    /// existe pero no se puede leer, se omite (mejor no deshacerlo que arriesgar
    /// borrarlo por error).
    fn record(&mut self, target: &Path) {
        if !self.active || self.pending.iter().any(|s| s.path == target) {
            return;
        }
        let before = if target.exists() {
            match std::fs::read(target) {
                Ok(bytes) => Some(bytes),
                Err(_) => return,
            }
        } else {
            None
        };
        // Línea base de la sesión: el PRIMER estado visto de este archivo.
        if !self.baseline.iter().any(|(p, _)| p == target) {
            self.baseline.push((target.to_path_buf(), before.clone()));
        }
        self.pending.push(FileSnapshot { path: target.to_path_buf(), before });
    }

    /// Archivos cuyo contenido actual difiere de su línea base de la sesión.
    fn changes(&self) -> Vec<FileChange> {
        let mut out = Vec::new();
        for (path, base_bytes) in &self.baseline {
            let now_bytes = std::fs::read(path).ok();
            if &now_bytes == base_bytes {
                continue; // sin cambio neto (p.ej. ya se hizo `/undo`)
            }
            let to_text = |b: &Vec<u8>| String::from_utf8_lossy(b).into_owned();
            out.push(FileChange {
                path: path.clone(),
                before: base_bytes.as_ref().map(to_text),
                now: now_bytes.as_ref().map(to_text),
            });
        }
        out
    }

    #[cfg(test)]
    fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Deshace el último grupo: devuelve `(restaurados, borrados)` o `None`.
    fn undo(&mut self) -> Option<(usize, usize)> {
        let group = self.stack.pop()?;
        let (mut restored, mut deleted) = (0usize, 0usize);
        // Orden inverso: el último cambio es el primero en revertirse.
        for snap in group.into_iter().rev() {
            match snap.before {
                Some(bytes) => {
                    if let Some(parent) = snap.path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if std::fs::write(&snap.path, &bytes).is_ok() {
                        restored += 1;
                    }
                }
                None => {
                    if snap.path.exists() && std::fs::remove_file(&snap.path).is_ok() {
                        deleted += 1;
                    }
                }
            }
        }
        Some((restored, deleted))
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

/// Bloquea el estado tolerando un posible envenenamiento del mutex (sin panic).
fn lock() -> std::sync::MutexGuard<'static, State> {
    match STATE.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Captura el estado anterior de `target` antes de que dpx lo modifique.
pub fn record_before(target: &Path) {
    lock().record(target);
}

/// Deshace el último grupo de cambios. `(restaurados, borrados)` o `None`.
pub fn undo() -> Option<(usize, usize)> {
    lock().undo()
}

/// Archivos que dpx cambió en la sesión (base vs. actual), para `/diff`.
pub fn session_changes() -> Vec<FileChange> {
    lock().changes()
}

/// Guarda RAII de un turno: abre al crearse, commitea al soltarse (cualquier
/// camino de salida de `run_turn`, incluido un early return).
pub struct TurnGuard;

impl TurnGuard {
    pub fn begin() -> Self {
        lock().begin();
        TurnGuard
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        lock().commit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Toda la lógica se prueba sobre una instancia LOCAL de `State`: sin estado
    // global, así no hay carrera con la ejecución en paralelo de cargo (ni con
    // los tests de run_turn, que usan el `State` global vía TurnGuard).
    #[test]
    fn ciclo_completo_restaura_y_borra() {
        let dir = std::env::temp_dir().join(format!("dpx-cp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let existente = dir.join("a.txt");
        let nuevo = dir.join("b.txt");
        std::fs::write(&existente, b"original\n").unwrap();

        let mut st = State::new();

        // Fuera de un turno (no activo), capturar es no-op.
        st.record(&existente);
        assert_eq!(st.depth(), 0);
        assert!(st.pending.is_empty());

        // Turno: capturar antes de tocar, luego modificar/crear.
        st.begin();
        st.record(&existente);
        st.record(&existente); // doble registro: el 1er estado manda
        st.record(&nuevo); // no existe → before = None
        assert_eq!(st.pending.len(), 2);
        std::fs::write(&existente, b"modificado\n").unwrap();
        std::fs::write(&nuevo, b"nuevo\n").unwrap();
        st.commit();
        assert_eq!(st.depth(), 1);

        // changes(): el existente (modificado) y el nuevo (creado) aparecen.
        let changes = st.changes();
        assert_eq!(changes.len(), 2);
        let exist_change = changes.iter().find(|c| c.path == existente).unwrap();
        assert_eq!(exist_change.before.as_deref(), Some("original\n"));
        assert_eq!(exist_change.now.as_deref(), Some("modificado\n"));
        let nuevo_change = changes.iter().find(|c| c.path == nuevo).unwrap();
        assert_eq!(nuevo_change.before, None); // era nuevo
        assert_eq!(nuevo_change.now.as_deref(), Some("nuevo\n"));

        // Undo: restaura el existente, borra el nuevo.
        assert_eq!(st.undo(), Some((1, 1)));
        assert_eq!(std::fs::read(&existente).unwrap(), b"original\n");
        assert!(!nuevo.exists());
        assert_eq!(st.depth(), 0);
        assert!(st.undo().is_none());

        // Tras el undo, todo volvió a la base → no hay cambios netos.
        assert!(st.changes().is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn turno_sin_cambios_no_apila() {
        let mut st = State::new();
        st.begin();
        st.commit();
        assert_eq!(st.depth(), 0);
    }
}
