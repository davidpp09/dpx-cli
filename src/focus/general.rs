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
    (
        "estrategia de testing",
        "USAR cuando: decidir qué testear, cómo organizar los tests, pirámide de testing, unit vs integración vs e2e, cobertura, qué NO testear, cómo decidir qué nivel de test usar para cada cosa. Palabras: testing, tests, unitario, integración, e2e, cobertura, coverage, pirámide, testear, TDD, qué testear, nivel de test. NO para escribir un test concreto — eso lo haces con tu criterio normal.",
        "1. PIRÁMIDE: Muchos tests UNITARIOS (lógica pura, baratos, rápidos), menos de INTEGRACIÓN (dos o más piezas reales hablando), pocos E2E (viajes críticos del usuario — solo lo que rompe el negocio si falla).\n\
         2. Qué TESTEAR: reglas de negocio, casos borde, contratos entre módulos, caminos tristes (errores, timeouts, inputs inválidos), serialización/parseo. El DOMINIO es el ROI más alto.\n\
         3. Qué NO testear: detalles de implementación (el cómo, no el qué), getters/setters triviales (0 lógica), código que ya testea el framework (rutas, configs), tests que solo inflan coverage pero no asertan nada real.\n\
         4. UNITARIOS: aíslan UNA unidad (función/método). Sin BD, sin red, sin disco. Rápidos. Si necesitas dobles, mockea el contrato (interfaz), no el objeto concreto.\n\
         5. INTEGRACIÓN: prueban que dos o más piezas reales hablan bien (repo↔BD, API↔middleware, serializer↔schema). Usan la dependencia real o una falsificación embebida (testcontainers, H2, SQLite en :memory:).\n\
         6. E2E: simulan un usuario real (navegador, CLI, HTTP). SOLO los viajes críticos (login, checkout, flujo principal). Frágiles y lentos: ten los mínimos que den confianza.\n\
         7. Cobertura = BRÚJULA, no medalla. 80%+ en dominio vale; 100% en todo es contraproducente. Mejor pocos tests con buenos asserts que muchos que pasan por encima.\n\
         8. Testea COMPORTAMIENTO, no estructura. Si refactorizas sin cambiar comportamiento los tests deben seguir pasando; si se rompen, estabas testeando implementación.",
    ),
    (
        "seguridad de aplicaciones (OWASP)",
        "USAR cuando: seguridad, proteger, sanitizar, validar input, XSS, SQL injection, autenticación, autorización, secretos, API keys, tokens, OWASP, vulnerabilidad, CSRF, CORS, hashing de contraseñas. Palabras: seguridad, sanitizar, validar, inyección, autenticación, autorización, secretos, token, OWASP, XSS, CSRF, CORS, bcrypt, JWT seguro. NO para configurar firewalls ni infraestructura (eso es DevSecOps).",
        "1. VALIDA Y SANITIZA en la frontera: todo input (query params, body, headers, cookies) es hostil hasta que se demuestre lo contrario. Usa validación tipada estricta al entrar (p.ej. zod, Pydantic, jakarta.validation); sanitiza (escapa) al renderizar, NUNCA al guardar. Regla: el dato en BD = crudo; al salir a HTML/JSON/SQL = escapado.\n\
         2. AUTENTICACIÓN ≠ AUTORIZACIÓN. Autenticación es QUIÉN eres (logins, sesiones, JWT/OAuth/OIDC). Autorización es QUÉ puedes hacer (RBAC/ABAC). NUNCA implementes auth desde cero: usa librerías maduras (OAuth2 + PKCE, next-auth, Spring Security, Devise). Contraseñas = bcrypt/argon2id (nunca SHA, nunca MD5). JWT: siempre con `exp`, audiencia (`aud`), y nunca datos sensibles en el payload.\n\
         3. SECRETOS fuera del código: usa variables de entorno + vault (`.env` SOLO en dev, NUNCA commiteado). El archivo `.env` vive en `.gitignore`. API keys, tokens de BD, secrets de JWT: todo por env vars o gestor de secretos (AWS Secrets Manager, Vault, Doppler). Revisa que no haya secrets en logs ni en el bundle del frontend.\n\
         4. RIESGOS OWASP TOP 10 de memoria: (1) Broken Access Control — verifica permisos en cada endpoint, no confíes en ocultar UI; (2) Cryptographic Failures — usa TLS 1.3, claves robustas, nada de cifrado casero; (3) Injection — usa consultas parametrizadas/prepared statements SIEMPRE (nada de concatenar SQL), y ORMs con cuidado (pueden generar SQL dinámico); (4) Insecure Design — threat-modeling al diseñar features sensibles; (5) Security Misconfiguration — defaults seguros, quita cabeceras de debug en prod; (6) Vulnerable Components — `npm audit`/`cargo audit`/dependabot en CI; (7) Auth Failures — 2FA, rate-limit en login, sesiones con timeout; (8) Software Integrity Failures — firma y checksum de dependencias, nada de CDN de terceros sin SRI; (9) Logging & Monitoring — logs SIN datos sensibles, alertas de anomalías; (10) SSRF — sanitiza URLs de usuario antes de fetchear, whitelist de dominios.\n\
         5. CABECERAS DE SEGURIDAD que nunca faltan: `Content-Security-Policy` (CSP) estricta sin `unsafe-inline`, `Strict-Transport-Security` (HSTS), `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`. CORS configurado con orígenes concretos, nunca `Access-Control-Allow-Origin: *` en prod. Verifica con `dpx:run` un linter de seguridad si el stack lo tiene (bandit para Python, eslint-plugin-security para JS, brakeman para Rails).",
    ),
    (
        "performance y optimización",
        "USAR cuando: optimizar, lento, performance, mejorar velocidad, tarda mucho, N+1, cuello de botella, profiling, carga lenta, slow. Palabras: optimizar, performance, lento, velocidad, profiling, cuello de botella, N+1, caché, índice. NO para refactorizar por legibilidad ni cambios de arquitectura sin métrica.",
        "1. MEDIR primero: profiler (Chrome DevTools para frontend, async-profiler/JFR para JVM, flamegraph/perf en backend). Sin datos, optimizas a ciegas. Identifica el bottleneck real, no el que imaginas.\n\
         2. OPTIMIZACIÓN PREMATURA es raíz de todo mal (Knuth). Optimiza SOLO el hot path confirmado con métrica. No sacrifiques legibilidad por micro-optimizaciones sin impacto medible.\n\
         3. Wins backend: N+1 queries (resuélvelas con batch/eager fetch/join fetch), índices que faltan, cache en caliente (Redis/local), paginación en consultas grandes, connection pooling bien configurado.\n\
         4. Wins frontend: bundle splitting + lazy loading (`React.lazy`/`dynamic()`/`import()`), imágenes responsive y lazy (`loading='lazy'` + `srcset`), debounce/throttle en scroll/input, evitar re-renders masivos (memo/useMemo selectivo).\n\
         5. Antes de cachear define política de invalidación: cache inconsistente es PEOR que lento. TTL + invalidación explícita. Mide hit rate post-cambio.\n\
         6. VERIFICA: reproduce el benchmark y muestra ANTES vs DESPUÉS con números, no con sensaciones.",
    ),
    (
        "accesibilidad web (a11y)",
        "USAR cuando: accesibilidad, a11y, WCAG, ARIA, lector de pantalla, teclado, contraste, foco visible, alt text, HTML semántico, screen reader, inclusivo. Palabras: accesibilidad, a11y, WCAG, ARIA, lector de pantalla, teclado, contraste, foco, alt, screen reader, role, aria-label. NO para SEO (aunque se solapan); NO para i18n/l10n.",
        "1. HTML SEMÁNTICO PRIMERO: usa `<header>`, `<main>`, `<nav>`, `<footer>`, `<article>`, `<section>`, `<button>`, `<input>` y headings `<h1>`–`<h6>` en orden sin saltos. El 80% de la accesibilidad es HTML correcto; ARIA es el 20% restante y solo cuando no hay elemento nativo.\n\
         2. ARIA CON MESURA: NO uses ARIA si el HTML ya lo da (p.ej. no pongas `role=\"button\"` en un `<button>`). Regla de oro: sin ARIA es mejor que ARIA mal usada. Si usas `aria-label`/`aria-labelledby`, verifica que el nombre accesible realmente se calcule bien.\n\
         3. NAVEGACIÓN POR TECLADO: todo interactivo se opera con Tab/Shift+Tab, se activa con Enter/Space, y se sale con Escape. El orden de tab sigue el DOM visual. NUNCA atrapes el foco sin salida (teclado-trampa). Los modales: foco al abrir → atrapado dentro → devuelto al cerrar.\n\
         4. FOCO VISIBLE SIEMPRE: `:focus-visible` con outline de al menos 2px y contraste ≥3:1 contra el fondo. NUNCA hagas `outline: none` sin poner un sustituto visible. El foco dice dónde estás; sin él, el teclado es inservible.\n\
         5. CONTRASTE WCAG AA: texto normal ≥4.5:1, texto grande (≥18px bold o ≥24px) ≥3:1, componentes interactivos y gráficos ≥3:1. Verifica con herramientas (axe, Lighthouse, browser DevTools). No confíes en el ojo.\n\
         6. ALT TEXT: toda `<img>` informativa lleva `alt` conciso (lo que un lector de pantalla diría en 1 frase). Imágenes decorativas: `alt=\"\"` (vacío, no ausente). SVG: `<title>` + `role=\"img\"` + `aria-labelledby`. Gráficos complejos: descripción larga en el contexto cercano, no en alt.\n\
         7. VERIFICA con herramientas reales: axe DevTools (linting automático), Lighthouse (puntuación a11y), y al menos UNA prueba con teclado real y lector de pantalla (VoiceOver/NVDA). Lo que compila puede ser inaccesible; solo la herramienta y la prueba humana lo cazan.",
    ),
    (
        "API REST: contratos y diseño",
        "USAR cuando: diseñar una API REST, endpoints, contratos HTTP, versionado, paginación, idempotencia, códigos de error, nombres de recursos, OpenAPI. Palabras: API, REST, endpoint, contrato, versionado, paginación, idempotente, status code, recurso, URL, OpenAPI, Swagger. NO para GraphQL ni RPC/gRPC.",
        "1. RECURSOS, no verbos: nombres en plural y sustantivos (`/orders`, `/users/{id}/orders`). Las acciones se modelan como sub-recursos (`POST /orders/{id}/cancellations`, no `POST /orders/{id}/cancel`).\n\
         2. CONTRATO explícito: define el schema con OpenAPI 3.x (tipos, ejemplos, errores). Request/response shapes CONSISTENTES en toda la API: mismo envelope, mismas reglas.\n\
         3. VERSIONADO desde el día uno (no «después lo añado»). Preferido: `/v1/...` en el path (simple y visible) o header `Accept: application/vnd.api+json;version=1`. NUNCA despliegues cambios que rompen sin bump de versión.\n\
         4. STATUS CODES con semántica HTTP real: 200 OK, 201 Created (+ `Location` header), 204 No Content, 400 Bad Request, 401 Unauthorized / 403 Forbidden, 404 Not Found, 409 Conflict, 422 Unprocessable, 429 Too Many Requests. NUNCA devuelvas 200 con `{\"error\": true}`.\n\
         5. ERRORES consistentes: cuerpo RFC 7807 Problem Detail (`type`, `title`, `status`, `detail`, `instance`). Errores de validación: array `errors` con `field` + `message`. NUNCA expongas stack traces, queries SQL ni detalles internos.\n\
         6. PAGINACIÓN: para colecciones usa cursor-based (`page[cursor]` + `Link` header con `rel=next`) si el dataset muta; offset (`page[number]`/`page[size]`) solo para datos estables. Metadatos de paginación EN el envelope, no en headers aislados.\n\
         7. IDEMPOTENCIA en mutaciones: `POST`/`PATCH` aceptan `Idempotency-Key`; si llega duplicada, devuelves la misma respuesta sin re-ejecutar. Crítico en pagos e integraciones.",
    ),
];
