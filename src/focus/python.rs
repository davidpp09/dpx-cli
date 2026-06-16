//! Focus Pack: Python (backend con FastAPI) — skills actualizadas a 2026.
//!
//! Se inyecta cuando el enfoque activo es `python`. El entrenamiento del modelo
//! suele arrastrar Python 3.8, Pydantic v1, pip+requirements y `@app.on_event`.

pub const SKILLS: &str = "\
# Enfoque activo: Python (backend con FastAPI)

Dominas Python moderno y FastAPI a nivel senior: APIs tipadas, async con criterio y el tooling
actual.

## VERSIONES ACTUALES (autoritativo · junio 2026 — CONFÍA en esto sobre tu memoria)
- **Python 3.13/3.14** son las líneas actuales; no propongas nada que exija <3.11. Sintaxis
  moderna de tipos: `str | None` (no `Optional[str]`), `list[str]` (no `List[str]`).
- **uv** (de Astral) es el gestor estándar: entornos, dependencias y lock con `pyproject.toml`
  (`uv add`, `uv run`, `uv sync`). No propongas pip+requirements.txt en proyectos nuevos, salvo
  que el proyecto ya funcione así.
- **ruff** para lint + format (sustituye a flake8/black/isort).
- **Pydantic v2 SIEMPRE**: `model_config`, `field_validator`, `model_dump()`. Los
  `@validator`/`.dict()` de v1 están obsoletos.
- **SQLAlchemy 2.0** estilo moderno: `Mapped[]` + `mapped_column()`, `select()`. Nada del Query
  API legacy.
- En FastAPI: **lifespan** (no `@app.on_event`, deprecado) y `Annotated[...]` en dependencias.
- Si no estás seguro de una versión EXACTA, dilo y verifica `pyproject.toml`. NUNCA inventes
  números de versión.

## FastAPI con criterio
- **Type hints en todo**: FastAPI ES sus tipos — validación, docs y serialización salen de ahí.
- Schemas Pydantic de entrada y salida SEPARADOS de los modelos de BD (`response_model` o tipo
  de retorno). Nunca expongas el modelo SQLAlchemy directo en la API.
- **Dependency injection con `Depends`** para sesión de BD, usuario actual, paginación y config.
  En tests se sobreescriben con `app.dependency_overrides`: por eso se inyecta, no se importa.
- Routers por dominio (`APIRouter`) montados en `main.py`, no un archivo gigante.
- **async def vs def, con criterio**: `async def` solo si TODO el I/O dentro es async (driver
  async, httpx); una llamada bloqueante dentro de un `async def` congela el event loop. Un `def`
  normal corre en threadpool y es la opción segura con librerías síncronas.

## Estructura típica
- `app/main.py` (app + lifespan), `app/routers/`, `app/services/`, `app/models/` (SQLAlchemy),
  `app/schemas/` (Pydantic), `app/core/config.py` con **pydantic-settings** (config tipada desde
  env; falla al arrancar si falta algo).

## Persistencia
- SQLAlchemy 2.0 + **Alembic** para migraciones versionadas, siempre. PostgreSQL en serio,
  SQLite para prototipos. Async con asyncpg/psycopg solo si el stack es async de punta a punta.

## Errores
- `HTTPException` para casos puntuales; **exception handlers globales**
  (`app.exception_handler`) que traducen errores de dominio a respuestas consistentes. Nunca un
  500 con traceback al cliente.

## Seguridad
- OAuth2 password flow + JWT (expiración corta) cuando hay auth propia; passwords con argon2 o
  bcrypt. CORS con allowlist explícita. Secretos por entorno, jamás en el repo.

## Testing
- **pytest** + `TestClient` (o `httpx.AsyncClient` + pytest-asyncio si es async): tests de
  endpoint contra la app real con dependencias sobreescritas (BD de test, usuario fake).
- Fixtures para la sesión de BD; una BD efímera (SQLite o Testcontainers) por suite.

## Run
- Desarrollo: `uv run uvicorn app.main:app --reload` (o `fastapi dev`). Producción: uvicorn con
  varios workers, detrás de un proxy.";

/// Playbooks EMPOTRADOS de Python: pasos A→B de las tareas que más se
/// repiten, para que dpx no explore a ciegas y aplique la convención correcta.
/// Se cargan cuando el focus activo es `python`. (nombre, cuándo, pasos).
pub const PLAYBOOKS: &[(&str, &str, &str)] = &[
    (
        "crear endpoint REST con FastAPI",
        "USAR cuando: crear/agregar un endpoint, API, ruta, router, GET/POST/PUT/DELETE. Palabras: FastAPI, APIRouter, endpoint, ruta, path operation. NO para tests sueltos ni config.",
        "1. Router en `app/routers/<dominio>.py` con `APIRouter(prefix=\"/api/...\", tags=[\"...\"])`; inyección por `Depends`.\n\
         2. NO metas lógica en el router: delega a un servicio en `app/services/`.\n\
         3. Request/Response como schemas Pydantic v2 (`BaseModel`, `model_config`) SEPARADOS de los modelos SQLAlchemy. Usa `response_model`.\n\
         4. Status codes correctos: `status.HTTP_201_CREATED` con `response_class=Response`; POST devuelve `Location` header.\n\
         5. Monta el router en `app/main.py` con `app.include_router(router)`.\n\
         6. Test con `TestClient` + `dependency_overrides` para aislar el servicio. Verifica con `uv run pytest`.",
    ),
    (
        "crear modelo SQLAlchemy + migración Alembic",
        "USAR cuando: agregar un modelo, tabla, entidad, base de datos, schema SQLAlchemy, migración. Palabras: modelo, tabla, SQLAlchemy, Base, Mapped, mapped_column, Alembic, migración.",
        "1. Modelo en `app/models/<nombre>.py` con `from sqlalchemy.orm import DeclarativeBase, Mapped, mapped_column` (estilo 2.0, NUNCA el Query API legacy).\n\
         2. `class MiModelo(Base): __tablename__ = \"...\"` con `Mapped[int]` + `mapped_column(primary_key=True)` y constraints.\n\
         3. Relaciones con `Mapped[list[\"Otro\"]]` + `relationship(...)`, NUNCA `backref` (usa `back_populates`).\n\
         4. SIEMPRE migración versionada: `alembic revision --autogenerate -m \"descripción\"` → revisa el script generado → `alembic upgrade head`.\n\
         5. NUNCA expongas el modelo en la API: crea un schema Pydantic separado en `app/schemas/`.\n\
         6. Test de repositorio/service que use el modelo; BD efímera (SQLite en :memory: o similar).",
    ),
    (
        "validar con Pydantic v2 (schemas + field_validator)",
        "USAR cuando: validar input, request body, schemas Pydantic, constraints, campos obligatorios, validación custom. Palabras: Pydantic, BaseModel, field_validator, model_config, validar, schema.",
        "1. Schema como clase `BaseModel`; usa `model_config = ConfigDict(from_attributes=True)` para ORM mode (nunca `class Config` de v1).\n\
         2. Tipos modernos: `str | None`, `list[str]`, `Annotated[int, Field(gt=0)]`. NUNCA `Optional[str]` ni `List[str]`.\n\
         3. Validación de formato con `Field(min_length=...)` o `EmailStr` (de pydantic-extra-types).\n\
         4. Validación de negocio con `@field_validator(\"campo\")` (método de clase); NUNCA `@validator` de v1.\n\
         5. Schemas de entrada y salida SEPARADOS: `CrearXRequest`, `XResponse`, `XUpdate`. Nunca mezcles create/update/response en uno.\n\
         6. En el endpoint, el body es el schema tipado: `def crear(body: CrearXRequest) -> XResponse`. FastAPI valida automáticamente y genera 422 si falla.",
    ),
    (
        "manejo global de errores en FastAPI",
        "USAR cuando: manejar excepciones, errores, 404/400/409, exception handlers, respuestas de error consistentes. Palabras: exception_handler, HTTPException, error, 404, 500, traceback.",
        "1. Define excepciones de dominio en `app/core/exceptions.py`: `class NotFoundError(Exception)`, `class ConflictError(Exception)`.\n\
         2. Handlers en `app/core/error_handlers.py` o en `main.py`: `@app.exception_handler(NotFoundError)` que devuelve `JSONResponse(status_code=404, content={...})`.\n\
         3. Cada handler mapea la excepción al status HTTP correcto y devuelve un cuerpo consistente: `{\"detail\": ..., \"type\": ..., \"status\": ...}`.\n\
         4. Un handler genérico para `Exception` → 500 loggea el traceback REAL (logging.exception) y al cliente solo \"Internal server error\". NUNCA traces al cliente.\n\
         5. `HTTPException` de FastAPI solo para casos puntuales; para lógica de negocio, usa tus propias excepciones y que el handler las traduzca.\n\
         6. Verifica: `uv run pytest -k error` y confirma que cada handler devuelve el body y status esperados.",
    ),
    (
        "test con pytest + TestClient + fixtures",
        "USAR cuando: escribir o pedir un test, probar un endpoint, pytest, TestClient, fixtures, dependency_overrides, test de integración. Palabras: test, pytest, TestClient, fixture, probar, testear.",
        "1. `TestClient(app)` de `fastapi.testclient` para tests síncronos; `httpx.AsyncClient` + pytest-asyncio si toda la app es async.\n\
         2. Sobreescribe dependencias con `app.dependency_overrides[get_db] = override_get_db` para inyectar una BD de test (SQLite :memory: o rollback por test).\n\
         3. Fixtures Pytest para la sesión de BD y el cliente: `@pytest.fixture def client(): ...` que crea tablas, yield el TestClient, y limpia.\n\
         4. Test del camino feliz: `response = client.post(\"/api/...\", json={...})` → `assert response.status_code == 201` y verifica el body JSON.\n\
         5. Test de borde: input inválido → 422, entidad no encontrada → 404, conflicto → 409. Un test por comportamiento, no por línea.\n\
         6. Corre `uv run pytest -v` y reacciona a la salida real; si un test falla, léelo antes de tocarlo.",
    ),
    (
        "config tipada con pydantic-settings",
        "USAR cuando: configuración, variables de entorno, settings, secretos, .env, configurar la app. Palabras: config, settings, .env, variables de entorno, pydantic-settings, secretos.",
        "1. `app/core/config.py` con `from pydantic_settings import BaseSettings`. Clase `Settings(BaseSettings)` con campos tipados.\n\
         2. `model_config = SettingsConfigDict(env_file=\".env\", env_file_encoding=\"utf-8\")`.\n\
         3. Agrupa settings relacionadas: `DATABASE_URL: str`, `SECRET_KEY: str`, `JWT_EXPIRATION_MINUTES: int = 30`, `CORS_ORIGINS: list[str] = [\"http://localhost:5173\"]`.\n\
         4. VALIDA al arrancar: `Settings()` falla si falta un campo sin default. Usa `@field_validator` para checks extra (ej. URL con formato).\n\
         5. Singleton: `@lru_cache def get_settings() -> Settings: return Settings()`. Se inyecta con `Depends(get_settings)`.\n\
         6. NUNCA hardcodees secretos ni los commitees; `.env` en `.gitignore`. En producción, variables de entorno reales sin archivo.",
    ),
    (
        "dependencias con Depends (DB, auth, paginación)",
        "USAR cuando: inyectar dependencias, sesión de BD, get_db, usuario actual, autenticación, autorización, paginación. Palabras: Depends, dependency, inyectar, get_db, get_current_user, sesión, autenticación.",
        "1. Dependencia de BD: `def get_db() -> Generator[Session, None, None]: ...` que crea sesión → yield → close. NUNCA imports globales de sesión.\n\
         2. Dependencia de usuario: `def get_current_user(token: str = Depends(oauth2_scheme), db: Session = Depends(get_db)) -> User:` que decodifica JWT y busca en BD.\n\
         3. Encadena dependencias: `get_current_active_user` depende de `get_current_user` → verifica `is_active`.\n\
         4. Paginación común: `def pagination(page: int = Query(1, ge=1), size: int = Query(20, ge=1, le=100)) -> dict:` → `{\"skip\": (page-1)*size, \"limit\": size}`.\n\
         5. Para tests: `app.dependency_overrides[get_db] = override_get_db` con una BD de test. Por eso se inyecta, no se importa.\n\
         6. Verifica: `uv run pytest` y confirma que las dependencias se pueden sobreescribir limpiamente en tests.",
    ),
];
