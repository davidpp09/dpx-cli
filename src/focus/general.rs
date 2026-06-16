//! Playbooks GENERALES (cross-stack): arquitectura, CSS/UI y lógica de negocio.
//! No dependen de un stack concreto, así que se cargan SIEMPRE (junto a los del
//! focus activo); el CLI inyecta el que encaje por similitud con la petición.
//! Buenas prácticas a 2026; si dudas de un dato que cambia con el tiempo,
//! `web_search` antes de afirmar.

pub const PLAYBOOKS: &[(&str, &str, &str)] = &[
    (
        "layout y CSS moderno",
        "USAR cuando: maquetar, layout, responsive, grid, flexbox, posicionar, columnas, diseño responsivo, CSS de estructura. Palabras: layout, grid, flex, responsive, columnas, maquetar. NO para animaciones ni el 'look' (ver «look premium»).",
        "1. Grid para layout 2D (filas Y columnas); Flexbox para 1D (una fila o una columna). Es sociedad, no o-uno-o-el-otro.\n\
         2. Espaciado SIEMPRE con `gap`, no márgenes manuales entre hijos.\n\
         3. Responsivo por COMPONENTE con `@container` (container queries), no solo media queries del viewport → componentes que se adaptan a su contexto.\n\
         4. Controla la cascada con `@layer` (base, components, utilities) en vez de pelear especificidad con `!important`.\n\
         5. Tokens con custom properties (`--space-*`, `--color-*`) para consistencia. `:has()` para estilos relacionales, `color-mix()` para variantes de color.",
    ),
    (
        "look premium y animaciones",
        "USAR cuando: que se vea caro/premium/profesional, animaciones, transiciones, micro-interacciones, hover, que se sienta pulido, una landing bonita. Palabras: animación, transición, premium, bonito, pulido, hover, caro. NO para layout estructural.",
        "1. Lo que hace que se vea CARO no es el color: es ESPACIADO generoso y consistente (escala de 4/8px), jerarquía tipográfica clara y alineación perfecta.\n\
         2. Anima SOLO `transform` y `opacity` (corren en GPU, 60fps); evita animar `width`/`top`/`margin` (causan reflow y se ven baratos).\n\
         3. Transiciones cortas (150–250ms) con easing natural (`ease-out`/`cubic-bezier`), nunca lineal. Micro-interacciones en hover/focus y al aparecer.\n\
         4. Consistencia: UN set de sombras, radios y duraciones (tokens). Detalles: gradientes sutiles, bordes de 1px, glassmorphism con mesura.\n\
         5. Respeta `@media (prefers-reduced-motion: reduce)`: reduce/apaga animaciones. La accesibilidad es parte del acabado premium.",
    ),
    (
        "arquitectura de una feature",
        "USAR cuando: diseñar la arquitectura, estructurar una feature/módulo, organizar capas, decidir clean/hexagonal/DDD/vertical-slice, dónde va cada cosa. Palabras: arquitectura, estructura, capas, módulo, diseño, organizar el código. NO para CSS ni para una función suelta.",
        "1. Empieza SIMPLE. Organiza por FEATURE (vertical slice: cada caso de uso con su UI→lógica→datos juntos) antes que por capas técnicas gigantes.\n\
         2. Regla de dependencias: el DOMINIO (reglas de negocio) NO depende de framework, BD ni UI; son los bordes los que dependen del dominio, nunca al revés.\n\
         3. NO sobre-ingenierices: hexagonal (puertos/adaptadores) y DDD táctico SOLO cuando hay muchas integraciones/persistencias o el dominio es complejo. Un CRUD no los necesita.\n\
         4. Una arquitectura simple y observable gana a una compleja a escala moderada. Añade capas cuando el dolor lo justifique, no antes.\n\
         5. Si dudas de un patrón o una versión actual, `web_search` — no inventes.",
    ),
    (
        "modelar la lógica de negocio",
        "USAR cuando: lógica de negocio, reglas de dominio, casos de uso, separar dominio de infraestructura, dónde poner las reglas. Palabras: lógica de negocio, dominio, reglas, caso de uso, service, negocio. NO para CSS ni layout.",
        "1. Separa el DOMINIO (reglas puras: precios, validaciones, transiciones de estado) de la INFRAESTRUCTURA (BD, red, UI). Las reglas no deben saber de SQL ni de HTTP.\n\
         2. Escribe las reglas como FUNCIONES PURAS donde se pueda: mismas entradas → misma salida, sin efectos. Triviales de testear.\n\
         3. Empuja los efectos (I/O) a los BORDES; el centro calcula, los bordes leen/escriben.\n\
         4. Valida en la frontera (dato crudo entra → dato válido y tipado sale); el dominio asume datos ya válidos.\n\
         5. Testea el dominio primero: son los tests más baratos y cazan los bugs que importan.",
    ),
    (
        "escribir funciones limpias",
        "USAR cuando: refactorizar una función, código difícil de leer, if anidados, función muy larga, demasiados parámetros, lógica enredada. Palabras: refactor, función larga, anidado, legible, limpiar, simplificar. NO para arquitectura de alto nivel.",
        "1. GUARD CLAUSES: maneja errores y casos borde al inicio con `return` temprano; deja el camino feliz plano y obvio. Mata las pirámides de if/else.\n\
         2. Una función hace UNA cosa, a un solo nivel de abstracción. Si necesitas comentar bloques internos, esos bloques son funciones aparte.\n\
         3. Sin efectos ocultos: que el nombre diga lo que hace; no mutes cosas que el llamador no espera.\n\
         4. Demasiados parámetros (4+) = falta una abstracción: agrúpalos en un objeto/struct.\n\
         5. Nombres que revelan la intención; el código se lee mucho más de lo que se escribe.",
    ),
];
