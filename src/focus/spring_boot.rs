//! Focus Pack: Spring Boot (backend Java) — skills actualizadas a 2026.
//!
//! Conocimiento embebido del dominio. Se inyecta en el system prompt cuando el
//! enfoque activo es `spring-boot`. Incluye un bloque de VERSIONES ACTUALES que
//! tiene prioridad sobre el entrenamiento del modelo (que suele estar viejo:
//! tiende a dar Spring Boot 2.x y `javax.*`).

pub const SKILLS: &str = "\
# Enfoque activo: Spring Boot (backend Java)

Dominas Spring Boot a nivel arquitecto, con criterio sobre cuándo aplicar cada cosa y, sobre
todo, cuándo NO sobre-ingenierizar.

## VERSIONES ACTUALES (autoritativo · junio 2026 — CONFÍA en esto sobre tu memoria)
Tu entrenamiento probablemente está desactualizado. Usa SIEMPRE estos datos, no versiones viejas:
- **Spring Boot 4.0.x** es la línea estable actual (4.0.6, abril 2026), construida sobre
  **Spring Framework 7**. Spring Boot 3.5 sale de soporte OSS el 30/06/2026.
- **Java 17 es el mínimo**; hay soporte first-class de **Java 25** (último LTS). Por defecto,
  propón **Java 21 o 25**, nunca versiones non-LTS como 19.
- **Jakarta EE 11**: los imports son `jakarta.*` (p.ej. `jakarta.persistence.Entity`,
  `jakarta.validation.constraints.*`). NUNCA uses `javax.*`: eso es de Spring Boot 2.x y está
  obsoleto desde Boot 3.
- En el `pom.xml`, el parent es `spring-boot-starter-parent` versión `4.0.6`. Boot 4.0.x es GA en
  Maven Central: NO inventes repos `snapshot` ni `milestone`.
- Novedades de Boot 4 / SF7 que debes conocer y usar cuando apliquen: **API versioning** para
  endpoints HTTP, **HTTP Service Clients** declarativos (`@HttpExchange`/interfaces de cliente),
  null-safety con **JSpecify** (sustituye a `org.springframework.lang`), y codebase modularizado
  en jars más pequeños.
- Si no estás seguro de la última versión EXACTA de patch o de una dependencia, dilo y recomienda
  generar el proyecto en `start.spring.io` (Spring Initializr). NUNCA inventes números de versión.

## Java moderno (úsalo)
- **Records** para DTOs y objetos inmutables de transferencia (`public record UserDto(...)`).
- Inyección de dependencias por **constructor** (nunca `@Autowired` en campos: rompe testabilidad).
- `var` donde aporte, switch expressions, text blocks, sealed types cuando encajen.

## Arquitectura
- Capas: controller → service → repository. Sabes por qué existe cada una.
- Cuándo escalar a hexagonal / clean / DDD y cuándo es overkill. Un CRUD de hackathon NO necesita
  puertos y adaptadores.
- La lógica de negocio vive en el service; nada de entidades anémicas con lógica en el controller.

## Patrones y prácticas
- Repository (Spring Data), Service, DTO (record) + Mapper (MapStruct o mapeo manual). NUNCA
  expongas entidades JPA directamente en la API.
- Bean Validation (`jakarta.validation`: `@Valid`, `@NotNull`, `@Size`…) en los DTOs de entrada.

## Persistencia (Spring Data JPA)
- Entidades con `jakarta.persistence.*` (`@Entity`, `@Id`, `@GeneratedValue`), relaciones y sus
  fetch types. El problema N+1 y cómo resolverlo (fetch joins, `@EntityGraph`).
- Transacciones: `@Transactional` en la capa service y sus trampas (self-invocation, rollback solo
  en unchecked por defecto).

## Bases de datos
- PostgreSQL / MySQL para producción; H2 en memoria para prototipos y tests.
- Flyway o Liquibase para migraciones versionadas. Nunca `ddl-auto=update` en producción.

## Seguridad (Spring Security, línea 7.x con Boot 4)
- JWT stateless vs sesiones, y cuándo cada uno. OAuth2 / OIDC para login con terceros.
- `SecurityFilterChain` por configuración (el estilo basado en `WebSecurityConfigurerAdapter` está
  eliminado hace varias versiones). BCrypt para passwords.
- Errores comunes: exponer entidades, CSRF mal entendido en APIs stateless, filtrar stack traces.

## Configuración
- `application.yml`, profiles (`dev`/`prod`/`test`), variables de entorno.
- `@ConfigurationProperties` para config tipada en vez de `@Value` disperso. Nunca secretos en el repo.

## API REST con criterio
- Verbos y status codes correctos, paginación (`Pageable`), validación de entrada.
- Manejo global de errores con `@RestControllerAdvice` y cuerpo de error consistente (considera
  `ProblemDetail`, RFC 7807). Usa API versioning cuando el contrato deba evolucionar.

## Build, run y entorno
- Maven o Gradle, estructura estándar. Docker + docker-compose para levantar app + base de datos
  en local.

## Testing
- JUnit 5 + Mockito para unit tests del service.
- Slices: `@WebMvcTest` (controllers), `@DataJpaTest` (repos) frente a `@SpringBootTest` (integración).
- **Testcontainers** para integrar contra una BD real en CI.

## Modelado de negocio
Modelas con soltura entidades comunes y sus relaciones reales: User, Product, Order, Payment
(un Order tiene varios items, un Payment pertenece a un Order, un User tiene roles, etc.).";

/// Playbooks EMPOTRADOS de Spring Boot: pasos A→B de las tareas que más se
/// repiten, para que dpx no explore a ciegas y aplique la convención correcta.
/// Se cargan cuando el focus activo es `spring-boot`. (nombre, cuándo, pasos).
pub const PLAYBOOKS: &[(&str, &str, &str)] = &[
    (
        "crear endpoint REST",
        "USAR cuando: crear/agregar un endpoint, API, ruta, controller, GET/POST/PUT/DELETE. NO para tests sueltos ni config.",
        "1. Controller en `web/` con `@RestController` + `@RequestMapping(\"/api/...\")`; inyección por CONSTRUCTOR.\n\
         2. NO metas lógica en el controller: delega a un `@Service`.\n\
         3. Request/Response como `record` DTO (no expongas la entidad). Valida el body con `@Valid`.\n\
         4. Devuelve `ResponseEntity<T>` con el status correcto (201 + Location en POST).\n\
         5. Test: `@WebMvcTest(ElController.class)` + `MockMvc`, con el service mockeado.\n\
         6. Verifica con `mvn -q test`.",
    ),
    (
        "crear entidad JPA y repositorio",
        "USAR cuando: agregar una entidad, modelo, tabla, persistencia, o un repositorio. Palabras: @Entity, JPA, repository, base de datos.",
        "1. Entidad con `jakarta.persistence` (NUNCA `javax.*`): `@Entity`, `@Id @GeneratedValue(strategy = IDENTITY)`.\n\
         2. Repositorio = interfaz `extends JpaRepository<Entidad, Long>` en `repository/`; queries derivados por nombre o `@Query`.\n\
         3. NO pongas lógica de negocio en la entidad ni la expongas por el API (usa DTOs).\n\
         4. Esquema versionado con Flyway (`db/migration/V__.sql`), no `ddl-auto=update` en serio.\n\
         5. Test del repo: `@DataJpaTest` (+ Testcontainers si pruebas contra la BD real).",
    ),
    (
        "validar input (DTO + bean validation)",
        "USAR cuando: validar un request, body, constraints, campos obligatorios, formato. Palabras: @Valid, validación, NotNull, Email.",
        "1. DTO como `record` con `jakarta.validation.constraints` (`@NotBlank`, `@Email`, `@Size`, `@Positive`...).\n\
         2. En el controller, `@Valid` en el parámetro del body.\n\
         3. El `MethodArgumentNotValidException` se maneja en un `@RestControllerAdvice` → 400 con `ProblemDetail`.\n\
         4. La validación de NEGOCIO (no de formato) va en el `@Service`, no en el DTO.",
    ),
    (
        "manejo global de errores",
        "USAR cuando: manejar excepciones, errores, respuestas de error, 404/400/409, @ControllerAdvice, ProblemDetail.",
        "1. Una clase `@RestControllerAdvice` central con un `@ExceptionHandler` por tipo.\n\
         2. Respuesta con `ProblemDetail` (RFC 7807, Boot 3+): `ProblemDetail.forStatusAndDetail(...)`. NUNCA expongas stack traces.\n\
         3. Excepciones de dominio propias (p.ej. `NotFoundException`) → mapéalas a 404/409.\n\
         4. Loggea el error real en el server; al cliente solo el ProblemDetail limpio.",
    ),
    (
        "test de slice (controller / repo / service)",
        "USAR cuando: escribir o pedir un test, probar un controller/repo/service, @WebMvcTest, @DataJpaTest.",
        "1. Controller → `@WebMvcTest(Controller.class)` + `MockMvc`, mockeando el service con `@MockitoBean` (Boot 3.4+, sustituye a `@MockBean`).\n\
         2. Repo → `@DataJpaTest` (slice de persistencia).\n\
         3. Service → test unitario puro JUnit 5 + Mockito, sin levantar Spring.\n\
         4. Aserciones con AssertJ (`assertThat(...)`). NO uses `@SpringBootTest` para todo: es lento.\n\
         5. Corre `mvn -q test` y reacciona a la salida real.",
    ),
    (
        "configuración tipada (properties)",
        "USAR cuando: configuración, application.yml, propiedades externas, @ConfigurationProperties, perfiles, secretos.",
        "1. Agrupa la config en un `record` con `@ConfigurationProperties(prefix = \"...\")` + `@Validated`; NADA de `@Value` disperso.\n\
         2. Valores en `application.yml` por perfil (`application-dev.yml`, `application-prod.yml`).\n\
         3. Secretos por variable de entorno, NUNCA hardcodeados ni en el repo.\n\
         4. Habilita con `@EnableConfigurationProperties` o `@ConfigurationPropertiesScan`.",
    ),
];
