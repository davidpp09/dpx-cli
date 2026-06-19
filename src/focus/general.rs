//! Playbooks GENERALES (cross-stack): arquitectura, CSS/UI y lógica de negocio.
//! No dependen de un stack concreto, así que se cargan SIEMPRE (junto a los del
//! focus activo); el CLI inyecta el que encaje por similitud con la petición.
//! Buenas prácticas a 2026; si dudas de un dato que cambia con el tiempo,
//! `web_search` antes de afirmar.

#[allow(dead_code)]
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
        "depuración sistemática",
        "USAR cuando: hay un bug, error, algo no funciona, falla, crashea, da resultado incorrecto, comportamiento raro/inesperado, debuggear, investigar por qué falla. Palabras: bug, error, falla, no funciona, crashea, debuggear, depurar, por qué falla, comportamiento inesperado, no anda. NO para añadir features nuevas ni para optimizar velocidad (ver «performance»).",
        "1. NO toques código al azar. Primero REPRODUCE el fallo de forma fiable: el caso mínimo que lo dispara. Lo que no puedes reproducir, no lo puedes arreglar NI confirmar que quedó.\n\
         2. FORMA UNA HIPÓTESIS concreta y falsable ('X pasa porque Y'), UNA a la vez. Sin hipótesis solo estás picando a ciegas.\n\
         3. AÍSLA por bisección: parte el problema a la mitad (logs/prints en puntos clave, comenta secciones, `git bisect`) hasta acorralar la línea/función culpable. Que hable la EVIDENCIA, no la intuición.\n\
         4. VERIFICA LA CAUSA RAÍZ, no el síntoma: cuando creas tenerla, explica el MECANISMO completo (por qué produce el síntoma). Si no lo puedes explicar, todavía no es la causa raíz.\n\
         5. ARREGLA la causa, no el síntoma. Un fix que 'hace que desaparezca' sin entender el porqué suele reintroducirse o tapar otro bug.\n\
         6. CONFIRMA: reproduce el caso original y comprueba que ahora pasa; añade un test que lo capture para que no regrese. Si tras 2 intentos sigues igual, RELEE en fresco y replantea la hipótesis — no insistas en la misma corazonada.",
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
    (
        "autenticación y autorización",
        "USAR cuando: login, autenticación, autorización, sesiones, JWT, OAuth, permisos, roles, RBAC, proteger rutas, quién puede hacer qué, refresh token, 2FA. Palabras: auth, login, sesión, JWT, OAuth, permisos, roles, RBAC, token, autenticar, autorizar, proteger ruta. NO para hashing genérico ni CORS (ver «seguridad OWASP»).",
        "1. AUTENTICACIÓN (quién eres) ≠ AUTORIZACIÓN (qué puedes). Sepáralas: una capa valida identidad, otra valida permisos en CADA acción.\n\
         2. NUNCA implementes auth desde cero: usa librerías maduras (Auth.js/next-auth, Spring Security, Passport, Devise, OAuth2+PKCE). Contraseñas = argon2id/bcrypt, jamás SHA/MD5.\n\
         3. Sesiones: token de acceso de vida CORTA + refresh token ROTATIVO. JWT con `exp` y `aud`, sin datos sensibles en el payload. Guárdalo en cookie httpOnly+Secure+SameSite, NO en localStorage.\n\
         4. Autorización en el SERVIDOR en cada endpoint (no confíes en ocultar la opción en la UI). RBAC (roles) para la mayoría; ABAC (atributos) si la regla depende del recurso concreto.\n\
         5. El default es DENEGAR: una ruta sin regla explícita está prohibida, no abierta.\n\
         6. 2FA y rate-limit en el login (frena fuerza bruta); el logout invalida el refresh token.\n\
         7. COMPONE con «API REST» (proteger endpoints) y «seguridad OWASP» (el resto de la superficie).",
    ),
    (
        "manejo de errores robusto",
        "USAR cuando: manejar errores, excepciones, qué pasa si falla, propagar errores, try/catch, Result, fallar de forma controlada, reintentos, timeouts, mensajes de error, no tragarse errores. Palabras: error, excepción, fallo, try, catch, throw, Result, reintento, retry, timeout, manejo de errores, fallback. NO para depurar un bug ya existente (ver «depuración sistemática»).",
        "1. Distingue errores ESPERADOS (input inválido, no encontrado, sin permiso) de INESPERADOS (bug, infra caída). Los esperados se modelan como valores/tipos; los inesperados se propagan y se loggean.\n\
         2. Falla TEMPRANO y ruidoso en desarrollo; suave y con mensaje útil en producción. NUNCA te tragues un error en silencio: un `catch {}` vacío es un bug oculto.\n\
         3. Propaga CON CONTEXTO, no lo pierdas: envuelve añadiendo qué intentabas ('al guardar el pedido: <causa>'). Usa Result/`?` (Rust), excepciones tipadas (Java/Py) o `Error.cause` (JS).\n\
         4. UN solo punto en cada BORDE (handler HTTP, main, UI) traduce el error a respuesta (status+mensaje) o a estado de error de la UI, SIN filtrar stack traces ni detalles internos al usuario.\n\
         5. Red/IO: timeout SIEMPRE + reintento con backoff SOLO si la operación es idempotente y el fallo es transitorio. Nunca reintentes a ciegas.\n\
         6. El mensaje al usuario dice QUÉ pasó y QUÉ hacer; el log técnico (causa + contexto) va aparte.\n\
         7. COMPONE con casi todo: cualquier skill que escriba lógica necesita esta para fallar bien.",
    ),
    (
        "capa de datos y persistencia",
        "USAR cuando: base de datos, persistencia, guardar datos, modelar tablas, queries, SQL, ORM, migraciones, transacciones, relaciones, índices, repositorio. Palabras: base de datos, BD, persistencia, tabla, query, SQL, ORM, migración, transacción, repositorio, índice, schema, DAO. NO para la lógica de negocio pura (ver «modelar la lógica de negocio»).",
        "1. Separa el ACCESO a datos (repositorios/DAOs) de la lógica de negocio: el dominio no escribe SQL, le pide a un repositorio. Así testeas el dominio sin BD.\n\
         2. Migraciones VERSIONADAS desde el día 1 (Flyway/Liquibase, Prisma migrate, Alembic, sqlx). El schema cambia SOLO por migración, nunca a mano; cada una reversible o con plan de rollback.\n\
         3. Transacción para operaciones que deben ser ATÓMICAS (varias escrituras que van juntas o no van). Mantenla CORTA: nada de IO externo dentro.\n\
         4. Evita el N+1: carga lo relacionado en UNA query (join/eager/`include`), no en un loop. Mídelo cuando la lista crece.\n\
         5. Índices en lo que filtras/ordenas/joineas; no de más (cuestan en escritura).\n\
         6. Consultas PARAMETRIZADAS siempre (nunca concatenes SQL); tipa al salir (datos crudos → objetos del dominio) y valida al entrar.\n\
         7. COMPONE con «API REST» (el endpoint pide al repositorio) y «modelar la lógica de negocio» (el dominio lo usa).",
    ),
    (
        "concurrencia y async",
        "USAR cuando: async, await, promesas, concurrencia, paralelismo, hilos, race condition, condición de carrera, bloqueo, lock, mutex, tareas en paralelo, idempotencia, colas, workers. Palabras: async, await, promesa, concurrencia, paralelo, hilo, thread, race, lock, mutex, idempotente, cola, worker, tarea en segundo plano. NO para optimizar velocidad en general (ver «performance»).",
        "1. async/await NO es paralelismo: es no-bloqueante. Para correr cosas EN PARALELO usa `Promise.all`/`tokio::join!`/`gather`; en serie solo si una depende de la otra.\n\
         2. Estado mutable COMPARTIDO entre tareas = condición de carrera. Evítalo: pasa datos por mensajes/canales, o protégelo con lock — pero un lock mal puesto serializa todo y mata el beneficio.\n\
         3. Toda espera tiene TIMEOUT y forma de CANCELAR; una tarea colgada no debe colgar el sistema.\n\
         4. Lo que puede reintentarse o llegar duplicado (webhooks, pagos, reintentos de red) debe ser IDEMPOTENTE: la misma operación dos veces deja el mismo efecto que una.\n\
         5. Trabajo pesado o diferible (emails, thumbnails, reportes) → fuera del request, a una cola/worker; no hagas esperar al usuario.\n\
         6. No metas algo BLOQUEANTE (CPU pesada, IO síncrono) dentro de un runtime async: lo congela. Aíslalo en un thread/pool aparte.\n\
         7. COMPONE con «performance» (paralelizar el hot path) y «capa de datos» (transacción + concurrencia).",
    ),
    (
        "observabilidad: logs y métricas",
        "USAR cuando: logging, logs, registrar, observabilidad, métricas, monitoreo, trazas, debugging en producción, qué loggear, niveles de log, structured logging, correlación. Palabras: log, logging, logger, observabilidad, métrica, monitoreo, traza, trace, registrar, nivel de log, correlación. NO para depurar localmente un bug (ver «depuración sistemática»).",
        "1. Logs ESTRUCTURADOS (campos: timestamp, nivel, mensaje, contexto), no `print` sueltos: así se buscan y filtran en producción.\n\
         2. Niveles con criterio: ERROR (algo se rompió, requiere acción), WARN (anómalo pero recuperado), INFO (eventos de negocio: 'pedido creado'), DEBUG (detalle de diagnóstico, apagado en prod).\n\
         3. Loggea los BORDES y las DECISIONES: entrada/salida de cada request (con id de correlación), errores con su causa, y los puntos donde el flujo se bifurca. NO dentro de loops calientes.\n\
         4. NUNCA loggees datos sensibles (contraseñas, tokens, PII, tarjetas): enmascara.\n\
         5. Un ID de CORRELACIÓN que viaje por todo el flujo: así reconstruyes qué pasó en UNA petición entre todo el ruido.\n\
         6. Métricas para lo que importa al negocio (latencia, tasa de error, throughput); alerta sobre umbrales, no sobre cada evento.\n\
         7. COMPONE con «manejo de errores robusto» (el error se loggea con contexto) y «depuración sistemática» (los logs son la evidencia).",
    ),
    (
        "estado y datos en el frontend",
        "USAR cuando: estado en el frontend, state management, manejar datos en React/Vue, fetch de datos, loading, error, empty states, caché en cliente, formularios, sincronizar con el servidor, stores. Palabras: estado, state, store, fetch, loading, cargando, data fetching, caché cliente, formulario, React Query, SWR, Zustand, sincronizar. NO para el layout visual (ver «layout y CSS moderno»).",
        "1. Distingue estado de SERVIDOR (datos que viven en la BD, los fetcheas) de estado de UI/CLIENTE (qué tab está abierto, el draft de un form). Mézclalos y sufres.\n\
         2. El estado de servidor se maneja con una librería de data-fetching (TanStack/React Query, SWR, RTK Query): te da caché, revalidación y loading/error gratis. NO lo metas a mano en un useState global.\n\
         3. Estado de UI: local (`useState`) si es de un componente; elevado/contexto solo si varios lo comparten; store global (Zustand/Redux/Pinia) SOLO para estado realmente global. Empieza local, sube si duele.\n\
         4. Maneja SIEMPRE los 4 estados de un dato remoto: loading, error, vacío (empty) y con datos. La UI que solo pinta el caso feliz se rompe en cuanto algo tarda o falla.\n\
         5. Formularios: estado controlado + validación en submit y en blur, error junto al campo, submit deshabilitado mientras envía. Usa React Hook Form/Formik para forms no triviales.\n\
         6. No dupliques el estado del servidor en cliente; deriva lo que puedas. Tras una mutación, invalida/revalida la query (no edites el caché a mano salvo optimistic update consciente).\n\
         7. COMPONE con «layout y CSS moderno»/«accesibilidad» (pintar los estados) y «API REST» (de dónde vienen los datos).",
    ),
    (
        "configuración y secretos por entorno",
        "USAR cuando: configuración, variables de entorno, env, .env, secretos, API keys, feature flags, config por entorno, 12-factor, settings, ajustes por ambiente dev/staging/prod. Palabras: config, configuración, env, variable de entorno, secreto, API key, feature flag, ambiente, entorno, settings, 12-factor. NO para vulnerabilidades (ver «seguridad OWASP») ni para el login (ver «autenticación y autorización»).",
        "1. Config FUERA del código: nada hardcodeado. Lee de variables de entorno (12-factor); valida al ARRANCAR que las requeridas existen y falla RUIDOSO si falta una (no a mitad de ejecución).\n\
         2. Secretos (keys, tokens, contraseña de BD) NUNCA en el repo: `.env` solo en dev y en `.gitignore`; en prod, un gestor (Vault, AWS Secrets Manager, Doppler) o las variables del entorno de despliegue.\n\
         3. Config POR AMBIENTE sin duplicar: una base + overrides por entorno. El código no pregunta 'en qué ambiente estoy' por todos lados; lee valores YA resueltos.\n\
         4. Centraliza y TIPA la config en UN objeto validado al inicio (zod/Pydantic/struct), no `process.env.X` esparcido por el código.\n\
         5. Feature flags para activar/desactivar sin desplegar; mantenlos efímeros (bórralos cuando la feature ya es estable, no los dejes pudrir).\n\
         6. Distingue config de BUILD (compile-time) de config de RUNTIME; los secretos JAMÁS en el bundle del frontend.\n\
         7. COMPONE con «seguridad OWASP» (manejo de secretos) y «autenticación y autorización» (claves de auth).",
    ),
    (
        "integración con servicios externos",
        "USAR cuando: integrar una API de terceros, cliente HTTP, llamar a un servicio externo, webhook, rate limit del proveedor, SDK, circuit breaker, fetch a otra API (Stripe, Twilio, etc.). Palabras: integración, API externa, tercero, cliente HTTP, webhook, rate limit, SDK, circuit breaker, servicio externo, proveedor. NO para diseñar TU propia API (ver «API REST»).",
        "1. Aísla cada servicio externo tras un CLIENTE/adaptador propio: el resto del código llama a TU interfaz, no al SDK crudo. Así lo cambias o lo mockeas sin tocar todo.\n\
         2. Toda llamada externa: timeout + reintento con backoff (si es idempotente) + manejo del fallo. La red SIEMPRE falla tarde o temprano; no asumas éxito.\n\
         3. Respeta los rate limits del proveedor (lee sus headers, espacía/encola las llamadas); cachea lo que no cambia para no re-pedirlo.\n\
         4. Circuit breaker para servicios flaky: si está caído, deja de martillarlo y degrada con gracia (valor por defecto, cola, mensaje claro) en vez de colgar todo.\n\
         5. Webhooks que RECIBES: verifica la firma, responde rápido (200) y procesa async; trátalos como idempotentes (llegan duplicados o fuera de orden).\n\
         6. Nunca confíes en la forma de la respuesta externa: valídala al entrar; que un cambio en su API no corrompa tu sistema.\n\
         7. COMPONE con «manejo de errores robusto» (fallar bien) y «concurrencia y async» (timeouts, colas, idempotencia).",
    ),
    (
        "documentación que sí sirve",
        "USAR cuando: documentar, README, docs, comentarios, ADR, docstrings, guía de uso, CHANGELOG, explicar una decisión de diseño en el repo. Palabras: documentar, documentación, README, comentario, ADR, docstring, CHANGELOG, guía, doc. NO para enseñarle un concepto al usuario (eso es tu rol de mentor normal).",
        "1. Documenta el PORQUÉ, no el qué: el código ya dice qué hace; los comentarios y docs explican decisiones, trade-offs y lo no-obvio. Un comentario que repite el código es ruido.\n\
         2. README con lo justo para EMPEZAR: qué es, cómo se corre, cómo se prueba, en menos de 2 minutos de lectura. Nada de novelas.\n\
         3. Para decisiones de arquitectura importantes, un ADR corto (contexto → decisión → consecuencias): el futuro-tú agradece saber por qué se eligió X.\n\
         4. API/funciones públicas: documenta el CONTRATO (entradas, salidas, errores que lanza, efectos), no la implementación. Docstrings/JSDoc en lo que otros consumen.\n\
         5. La doc vive CON el código y se actualiza en el MISMO cambio; doc desincronizada miente y es peor que no tenerla.\n\
         6. Ejemplos > prosa: un fragmento de uso real enseña más que tres párrafos.\n\
         7. COMPONE con «arquitectura de una feature» (ADRs) y «API REST» (documentar el contrato).",
    ),
    (
        "refactorización segura",
        "USAR cuando: refactorizar sin cambiar comportamiento, mover código, extraer una función/módulo, reorganizar, limpiar deuda técnica sin romper, reestructurar. Palabras: refactor, refactorizar, mover, extraer, reorganizar, reestructurar, deuda técnica, limpiar sin romper. NO para escribir una función nueva legible (ver «escribir funciones limpias») ni para arreglar un bug (ver «depuración sistemática»).",
        "1. Refactor = cambiar la FORMA sin cambiar el COMPORTAMIENTO. Si cambias comportamiento, eso es feature/fix, no refactor — NO los mezcles en el mismo cambio.\n\
         2. Red de seguridad PRIMERO: que haya tests cubriendo el comportamiento actual; si no, escríbelos ANTES de tocar. Así sabes que no rompiste nada.\n\
         3. Pasos PEQUEÑOS y verdes: cada micro-cambio compila y pasa tests antes del siguiente. Nada de refactors gigantes de una sentada que dejan todo roto a la mitad.\n\
         4. Para renombrar o mover un símbolo usado en varios sitios, usa `rename_symbol` (exacto, cross-file) o `find_references` antes de tocar a mano: editar a ciegas deja referencias colgando.\n\
         5. Patrones seguros, uno a la vez: extraer función (nombra un bloque), extraer módulo, condicional anidado → guard clauses, introducir parámetro/objeto.\n\
         6. Commitea el refactor APARTE de cambios de comportamiento, para que el diff sea revisable y reversible.\n\
         7. COMPONE con «escribir funciones limpias» (el objetivo) y «estrategia de testing» (la red de seguridad).",
    ),
    (
        "gestión de dependencias",
        "USAR cuando: añadir una dependencia, elegir librería, gestionar deps, package.json/Cargo.toml/requirements, lockfile, auditar dependencias, actualizar versiones, vendoring, sub-dependencias. Palabras: dependencia, librería, dep, paquete, package, lockfile, npm, cargo, pip, auditar, actualizar versión, vendoring. NO para diseñar la arquitectura del módulo (ver «arquitectura de una feature»).",
        "1. Antes de añadir una dep, pregúntate si vale la pena: ¿resuelve un problema real y difícil, o son 10 líneas que escribes tú? Cada dep es superficie de ataque, peso y mantenimiento.\n\
         2. Elige por SALUD, no por estrellas: mantenida (commits recientes), pocas sub-deps, licencia compatible, sin vulnerabilidades conocidas.\n\
         3. Lockfile SIEMPRE commiteado (package-lock/Cargo.lock/poetry.lock) → builds reproducibles. Versiones con rango sensato, nunca `*`.\n\
         4. Audita regularmente (`npm audit`/`cargo audit`/dependabot) y actualiza con INTENCIÓN: lee el changelog de un bump mayor, no actualices a ciegas.\n\
         5. Una dep que usas en UN sitio trivial = candidata a quitar. Menos deps = menos cosas que se rompen solas.\n\
         6. Un solo gestor por proyecto; evita la misma dep en dos versiones distintas.\n\
         7. COMPONE con «seguridad OWASP» (auditar) y «arquitectura de una feature» (qué construir vs traer).",
    ),
    (
        "git y flujo de cambios",
        "USAR cuando: git, commit, branch, rama, pull request, PR, merge, conflicto, historial, versionar cambios, .gitignore, rebase, stash. Palabras: git, commit, branch, rama, PR, pull request, merge, conflicto, rebase, gitignore, historial, stash. NO para versionar una API pública (ver «API REST»).",
        "1. Commits ATÓMICOS: uno por cambio lógico coherente, que compila y pasa tests por sí solo. Nada de un commit 'arreglos varios' que mezcla cinco cosas.\n\
         2. Mensaje que explica QUÉ y POR QUÉ, en imperativo ('añade validación de email'), no 'cambios' ni 'wip'. El historial es documentación.\n\
         3. Rama por feature/fix; mantenla CORTA y al día con la base (rebase/merge frecuente) para evitar conflictos monstruo.\n\
         4. Antes de un PR: diff limpio (sin prints de debug ni archivos basura), tests verdes, y descripción de qué y por qué.\n\
         5. Conflictos: entiende AMBOS lados antes de resolver; no descartes el cambio del otro a ciegas. Tras resolver, RE-CORRE los tests.\n\
         6. `.gitignore` desde el inicio (build, deps, secretos, caches); NUNCA commitees `.env`, `node_modules` ni `target`.\n\
         7. COMPONE con «refactorización segura» (commit aparte del refactor) y la disciplina de cambio mínimo.",
    ),
    (
        "construir con IA / LLMs",
        "USAR cuando: construir con IA, LLM, integrar un modelo de lenguaje, prompt, OpenAI/Anthropic/etc., tokens, streaming de respuesta, RAG, embeddings, ventana de contexto, agente, function calling. Palabras: IA, LLM, modelo de lenguaje, prompt, token, streaming, RAG, embedding, contexto, agente, OpenAI, Anthropic, inferencia. NO para los gates internos de dpx.",
        "1. Trata al LLM como un componente NO determinista y falible: valida/parsea su salida (esquema), maneja que devuelva basura y ten un fallback. Nunca asumas que la respuesta es correcta o bien formada.\n\
         2. El CONTEXTO es el recurso caro: mete solo lo relevante (recupera, no vuelques todo); vigila la ventana y el costo por tokens. Poco contexto bueno > mucho contexto ruidoso.\n\
         3. Streaming para respuestas largas (mejor UX); maneja cortes a mitad y reintentos. Timeouts como cualquier llamada externa.\n\
         4. Prompts versionados y con EVALS: un cambio de prompt ES un cambio de comportamiento — ten casos de prueba que midan si sigue haciendo lo correcto antes de tocarlo.\n\
         5. RAG cuando necesitas conocimiento externo/actual: chunking sensato + embeddings + recuperar top-k relevante; la calidad del retrieve manda sobre el modelo.\n\
         6. Key por entorno, rate limit y costo monitoreado; NUNCA expongas la API key en el cliente.\n\
         7. COMPONE con «integración con servicios externos» (el LLM ES un servicio externo), «manejo de errores robusto» y «configuración y secretos por entorno».",
    ),
    (
        "diseño con carácter (no genérico)",
        "USAR cuando: que NO se vea genérico, que tenga personalidad/carácter, diseño distintivo, que no parezca hecho por IA, dirección de arte, identidad visual, elegir una estética, que se vea hecho a propósito y no de plantilla. Palabras: carácter, personalidad, distintivo, no genérico, identidad, dirección de arte, estética, único, AI slop, plantilla. NO para la técnica de animación/espaciado (ver «look premium y animaciones») ni para la estructura (ver «layout y CSS moderno»).",
        "1. El default de toda IA (fondo oscuro + gradientes morados/neón + animaciones por todos lados + glassmorphism) está QUEMADO: grita 'generado sin pensar'. Si quieres carácter, EMPIEZA descartando ese molde.\n\
         2. Toma UNA decisión de arte fuerte y comprométete: una estética clara (editorial/cálida · minimal monocroma · retro/brutalista · técnica tipo Charm-CLI…) ejecutada con coherencia. El carácter viene de la CONVICCIÓN, no de mezclar tendencias.\n\
         3. La TIPOGRAFÍA es el 80% del carácter: una fuente con personalidad (un serif editorial, un grotesque con actitud, un mono con gracia) define más que cualquier color. No uses la Inter de siempre por inercia.\n\
         4. PALETA con intención: 1-2 colores con personalidad (NO el azul/morado corporativo por defecto), aplicados con consistencia. A veces monocromo + UN acento dice más que un arcoíris de gradientes.\n\
         5. Detalles que dan carácter sin ruido: bordes/esquinas con criterio (la CLI de Charm = redondeados suaves + padding generoso), microcopy con voz propia, un ícono/ilustración hecho a mano en vez de stock, asimetría intencional.\n\
         6. MENOS movimiento, mejor: una o dos animaciones CON propósito > todo animado. El exceso de motion es la firma del template genérico.\n\
         7. Norte de buen gusto: Charm (charm.sh) en terminal; en web, lo editorial/cálido/opinionado por encima del dark-startup-de-IA. COMPONE con «look premium y animaciones» (la técnica del pulido) y «layout y CSS moderno» (la estructura).",
    ),
];
