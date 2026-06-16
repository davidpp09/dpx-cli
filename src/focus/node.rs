//! Focus Pack: Node.js (backend JavaScript/TypeScript) — skills actualizadas a 2026.
//!
//! Se inyecta cuando el enfoque activo es `node`. El entrenamiento del modelo
//! suele arrastrar CommonJS, callbacks y Express 4 con patrones de 2019.

pub const SKILLS: &str = "\
# Enfoque activo: Node.js (backend JavaScript/TypeScript)

Dominas el backend en Node a nivel senior: APIs sólidas, async sin sustos y criterio para no
sobre-armar un servicio pequeño.

## VERSIONES ACTUALES (autoritativo · junio 2026 — CONFÍA en esto sobre tu memoria)
- **Node 24 es la línea LTS activa** (22 en mantenimiento). Nada de ejemplos con Node 16/18.
- **ESM por defecto** (`\"type\": \"module\"`): `import`/`export`, no `require` en código nuevo.
  Builtins con prefijo **`node:`** (`import { readFile } from 'node:fs/promises'`).
- Ya vienen integrados: `fetch`, `AbortController`, el test runner **`node --test`**, `--watch`
  y `--env-file`. No instales dependencias para lo que Node ya trae.
- **TypeScript primero**: Node moderno ejecuta `.ts` directo (type stripping) para scripts y
  desarrollo; para producción compila con `tsc` o usa `tsx`. Chequeo de tipos en CI con
  `tsc --noEmit`.
- Frameworks HTTP: **Fastify 5** (default recomendado: rápido, validación por schema, TS de
  primera) o **Express 5** (ubicuo; ya maneja errores en handlers async). Hono si es
  edge/serverless.
- Si no estás seguro de una versión EXACTA, dilo y verifica el `package.json`. NUNCA inventes
  números de versión.

## Arquitectura de un servicio
- Capas: route/handler → service (negocio) → repositorio/cliente externo. El handler traduce
  HTTP, no contiene lógica de negocio.
- **Valida TODO el input en el borde con zod** (body, params, query y env). Los tipos se
  infieren del schema: una sola fuente de verdad.
- Config tipada: las variables de entorno se parsean y validan al arrancar (falla rápido si
  falta una), nunca `process.env.X` regado por el código. Secretos fuera del repo.

## Async sin sustos
- `async/await` siempre; `Promise.all` para trabajo paralelo independiente.
- Toda promesa se espera o se maneja: un rejection sin catch TUMBA el proceso en Node moderno.
- Timeouts y cancelación con `AbortSignal` en toda llamada saliente.
- El event loop no se bloquea: CPU-bound → `worker_threads`; archivos grandes → streams.

## Errores
- Clases de error de dominio (NotFound, Validation, Conflict) y un manejador central (error
  handler de Express / `setErrorHandler` de Fastify) que las traduce a status codes y a un
  cuerpo de error consistente. Nunca filtres stack traces al cliente.

## Persistencia
- **Prisma o Drizzle** (TS-first) según el proyecto; SQL directo con `pg` es respetable en
  servicios chicos. **Migraciones versionadas siempre**, nunca tocar el schema a mano.

## Seguridad mínima de toda API
- CORS con allowlist explícita, rate limiting, helmet (o equivalente), passwords con
  argon2/bcrypt y JWT de expiración corta si hay auth propia.

## Testing
- **Vitest** (o `node --test`) para unit tests del service. Para HTTP: `app.inject()` de
  Fastify o supertest en Express (sin levantar puerto). Testcontainers cuando haga falta una
  BD real.

## Tooling
- **pnpm** como package manager por defecto; **Biome** (o ESLint+Prettier si ya están) para
  lint/format. Scripts npm claros: `dev`, `build`, `test`, `start`.";

/// Playbooks EMPOTRADOS de Node.js: pasos A→B de las tareas que más se
/// repiten, para que dpx no explore a ciegas y aplique la convención correcta.
/// Se cargan cuando el focus activo es `node`. (nombre, cuándo, pasos).
pub const PLAYBOOKS: &[(&str, &str, &str)] = &[
    (
        "crear endpoint REST con Fastify 5",
        "USAR cuando: crear un endpoint, ruta, handler, API REST, POST/GET/PUT/DELETE, Fastify. Palabras: ruta, endpoint, handler, Fastify, API. NO para WebSocket ni GraphQL.",
        "1. Crea el handler en `src/routes/<recurso>.ts` (ESM, TypeScript estricto). Registra con `fastify.route({ method, url, handler })`, o auto-carga rutas con `@fastify/autoload`.\n\
         2. El handler recibe `(request: FastifyRequest, reply: FastifyReply)`. Extrae params con `request.params`, query con `request.query`, body con `request.body`.\n\
         3. Valida TODO input con schema de Fastify (que usa zod/ajv): `schema: { body: zSchema, params: zSchema, querystring: zSchema }`. Fastify lo rechaza con 400 automáticamente.\n\
         4. Lógica en un SERVICE aparte (`src/services/<recurso>.ts`), NUNCA en el handler. El handler solo traduce HTTP: recibe → llama al service → responde.\n\
         5. `reply.code(201).send(data)` para POST, `reply.send(data)` para GET. Tipa el body de respuesta con el tipo inferido del service, no `any`.\n\
         6. Verifica: `pnpm run dev`, prueba con curl/Postman el happy path y el 400 por body inválido.",
    ),
    (
        "modelo de datos con Prisma + migración",
        "USAR cuando: crear modelo/schema de BD, Prisma, entidad/tabla, migración, ORM. Palabras: modelo, schema, tabla, Prisma, migración, entidad, migrate. NO para SQL directo sin ORM.",
        "1. Define el modelo en `prisma/schema.prisma`: `model User { id Int @id @default(autoincrement()) ... }`. Usa `@map` y `@@map` para nombres snake_case en BD.\n\
         2. Relaciones con campos en AMBOS lados: `posts Post[]` en User y `author User @relation(fields: [authorId], references: [id])` + `authorId Int` en Post. NUNCA crees FK manuales.\n\
         3. Migración: `npx prisma migrate dev --name add-<entidad>`. NUNCA edites la BD a mano. Commitea `prisma/migrations/`.\n\
         4. Cliente singleton en `src/db.ts`: `export const prisma = new PrismaClient()`. Usa un solo `prisma` en toda la app. Cierra en shutdown: `await prisma.$disconnect()`.\n\
         5. Consultas en el service: `prisma.user.findMany({ where, include, select })`. Usa `select` para no traer campos de más; `include` para relaciones necesarias.\n\
         6. Verifica: `npx prisma studio` para inspeccionar datos; `pnpm test` con BD de test (SQLite :memory:) si hay tests.",
    ),
    (
        "validar input con zod",
        "USAR cuando: validar body/query/params, schema de validación, zod, input de usuario, DTO, validación de formulario del backend. Palabras: validar, zod, schema, body, input, DTO.",
        "1. Define schemas en `src/schemas/<recurso>.ts`: `export const createUserSchema = z.object({ name: z.string().min(2).max(100), email: z.string().email() })`. Tipos con `z.infer<typeof ...>`.\n\
         2. Para Fastify: usa `fastify-type-provider-zod` o pasa el schema zod convertido a JSON Schema con `zod-to-json-schema` en la opción `schema` de la ruta.\n\
         3. Si usas Express 5, valida en el handler con `createUserSchema.parse(req.body)` (lanza ZodError). Maneja el error en el error handler central (ver playbook de errores).\n\
         4. Valida TAMBIÉN las variables de entorno con zod en `src/config.ts` (ver playbook de config tipada).\n\
         5. NUNCA valides a mano con if/else; NUNCA uses `as Type` sin validar. Zod es la UNICA fuente de verdad de la forma de los datos.\n\
         6. Verifica: `pnpm test` con casos de body vacío, campos faltantes, formatos inválidos → 400.",
    ),
    (
        "manejo global de errores en Fastify",
        "USAR cuando: manejar errores, exception handler, error handler, respuestas de error consistentes, 404/400/409/500. Palabras: error, exception, handler, errorHandler, setErrorHandler, status code.",
        "1. Define errores de dominio en `src/errors.ts`: `class NotFoundError extends Error { statusCode = 404 }`, `class ConflictError extends Error { statusCode = 409 }`.\n\
         2. Registra el handler GLOBAL con `fastify.setErrorHandler((error, request, reply) => { ... })`. Fastify 5 soporta handlers async.\n\
         3. En el handler: si el error tiene `statusCode`, `reply.code(error.statusCode).send({ error: error.message })`. Si es `ZodError` → `reply.code(400).send({ error: 'Validation failed', details: ... })`.\n\
         4. Error genérico: loggea el error real (`fastify.log.error(error)`) y al cliente `{ error: 'Internal Server Error' }`. NUNCA traces ni stacks al cliente.\n\
         5. Los servicios LANZAN errores de dominio (`throw new NotFoundError(...)`) en vez de devolver null/undefined. El handler los traduce a HTTP sin lógica de HTTP en el service.\n\
         6. Verifica: `pnpm test` → manda requests que disparen cada tipo de error y confirma status code y cuerpo.",
    ),
    (
        "test con Vitest + Fastify inject",
        "USAR cuando: escribir o pedir un test, probar un endpoint, Vitest, test runner, Fastify inject, app.inject, test de integración. Palabras: test, Vitest, probar, testear, inject, Fastify.",
        "1. Usa Vitest (`pnpm add -D vitest`). Config en `vitest.config.ts`: `defineConfig({ test: { globals: true } })`.\n\
         2. Para tests HTTP: `app.inject()` de Fastify (sin levantar puerto real). Construye la app en un helper `buildApp()` que puedas usar en tests y en server.ts.\n\
         3. Separa `app.ts` (crea la instancia de Fastify con rutas y plugins) de `server.ts` (solo `app.listen()`). Así los tests importan `app` sin escuchar puerto.\n\
         4. Test del camino feliz: `const res = await app.inject({ method: 'POST', url: '/api/users', payload: { ... } })` → `expect(res.statusCode).toBe(201)`.\n\
         5. Test de bordes: body inválido → 400, recurso no encontrado → 404, conflicto → 409. Mockea Prisma con `vitest-mock-extended` o usa BD en memoria.\n\
         6. Corre `pnpm vitest run` y reacciona a la salida real. Un test por comportamiento, no por línea de código.",
    ),
    (
        "config tipada con zod validando env",
        "USAR cuando: configuración, variables de entorno, settings, .env, process.env, dotenv. Palabras: config, env, .env, variables de entorno, settings, dotenv.",
        "1. `src/config.ts` con un schema zod: `const envSchema = z.object({ PORT: z.coerce.number().default(3000), DATABASE_URL: z.string().url(), JWT_SECRET: z.string().min(32) })`.\n\
         2. Carga `.env` con `dotenv` (o `--env-file` de Node 24). Parseo: `const env = envSchema.parse(process.env)`. FALLA rápido si falta algo.\n\
         3. Exporta el objeto parseado: `export const config = { port: env.PORT, db: { url: env.DATABASE_URL }, jwt: { secret: env.JWT_SECRET } }`.\n\
         4. NUNCA uses `process.env.X` desperdigado en el código. Importa el objeto `config` tipado, que tiene defaults y validación.\n\
         5. `.env` en `.gitignore`; `.env.example` con valores dummy para documentar qué variables hacen falta.\n\
         6. Verifica: arranca con `pnpm dev` sin `.env` y confirma que falla con mensaje claro; con `.env` correcto, arranca sin errores.",
    ),
    (
        "middleware de auth con JWT en Fastify",
        "USAR cuando: autenticación, JWT, proteger rutas, login, signup, middleware auth, token, @fastify/jwt. Palabras: auth, JWT, token, login, autenticación, proteger, middleware, preHandler.",
        "1. Registra `@fastify/jwt` con `fastify.register(fastifyJwt, { secret: config.jwt.secret })`. El plugin añade `fastify.jwt` y el decorador `request.jwtVerify()`.\n\
         2. Login: endpoint que recibe credenciales, verifica con argon2/bcrypt, y firma: `const token = fastify.jwt.sign({ sub: user.id, role: user.role }, { expiresIn: '15m' })`. Refresh token con expiración más larga.\n\
         3. Protege rutas con `preHandler: [fastify.authenticate]` (un decorador que registras: `fastify.decorate('authenticate', async (req, reply) => { await req.jwtVerify() })`).\n\
         4. El usuario autenticado va en `request.user` (tipado con `fastify.jwt.decode<{ sub: number }>()`). Declara el tipo en `fastify.d.ts` para TypeScript.\n\
         5. Roles: añade `role` al payload JWT y un segundo middleware `fastify.authorize('admin')` que verifica `request.user.role`.\n\
         6. Verifica: test que ruta sin token → 401, token expirado → 401, token válido → 200, rol incorrecto → 403.",
    ),
];
