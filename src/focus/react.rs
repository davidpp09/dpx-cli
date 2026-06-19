//! Focus Pack: React (frontend) — skills actualizadas a 2026.
//!
//! Se inyecta cuando el enfoque activo es `react`. Incluye un bloque de
//! VERSIONES ACTUALES autoritativo: el entrenamiento del modelo suele arrastrar
//! React 17/18, create-react-app y patrones ya muertos.

pub const SKILLS: &str = "\
# Enfoque activo: React (frontend)

Dominas React a nivel senior: sabes qué patrón aplica en cada caso y, sobre todo, cuándo NO
añadir complejidad (estado global, librerías, abstracciones) que el proyecto no necesita.

## VERSIONES ACTUALES (autoritativo · junio 2026 — CONFÍA en esto sobre tu memoria)
- **React 19.x** es la línea estable: Actions y form actions, `use()`, `useOptimistic`,
  `useActionState`, Server Components (en frameworks que los soportan) y `ref` como prop normal
  (sin `forwardRef` en código nuevo).
- **create-react-app está MUERTO** (deprecado en 2025). NUNCA lo propongas. SPA nueva → **Vite**.
  Fullstack/SSR → Next.js (App Router) o React Router v7 en modo framework.
- **React Compiler** (memoización automática) es estable: en proyectos que lo usan, los
  `useMemo`/`useCallback` manuales sobran casi siempre. Si el proyecto no lo usa, memoiza solo
  cuando haya un problema de rendimiento medido.
- **TypeScript por defecto** en todo código nuevo. Nada de `any` gratuito.
- Si no estás seguro de la versión EXACTA de una librería, dilo y verifica el `package.json`
  (lo tienes en el árbol del proyecto). NUNCA inventes números de versión.

## Estado con criterio (el error nº1)
- **Server state ≠ client state.** Datos que vienen de una API: **TanStack Query** (cache,
  revalidación, estados de loading/error), no `useEffect` + `useState` artesanal.
- Estado local (`useState`) primero. Global solo cuando de verdad se comparte: **Zustand** o
  Context (Context para datos estables tipo theme/usuario, no para estado que cambia mucho).
- Redux solo si el proyecto ya lo tiene; no lo introduzcas en proyectos nuevos.
- El estado derivado se CALCULA en el render, no se duplica en otro `useState`.

## useEffect: para sincronizar con sistemas externos, punto
- Anti-patrón clásico: `useEffect` para derivar estado o \"reaccionar\" a props — casi siempre
  sobra. Mantra: si se puede calcular en render, no es un effect.
- Cuándo sí: subscripciones, timers, APIs del navegador, librerías no-React. Siempre con cleanup
  y dependencias honestas (nada de silenciar el linter).

## Componentes y composición
- Componentes pequeños; composición (`children`, slots) sobre prop drilling.
- Custom hooks para extraer lógica con estado reutilizable; funciones puras para lo demás.
- Keys estables en listas (nunca el índice si la lista cambia de orden o se filtra).
- Code splitting con `lazy` + `Suspense` en rutas y bloques pesados.

## Formularios
- **react-hook-form + zod** para validación seria, o form actions de React 19
  (`useActionState`) cuando el framework lo soporte. Nada de un `useState` por campo en
  formularios grandes.

## Estructura
- Organiza por **feature**, no por tipo de archivo, en cuanto el proyecto crece
  (`features/orders/{components,hooks,api}` mejor que un `components/` gigante).

## Testing
- **Vitest + React Testing Library**: testea comportamiento visible (roles, texto), no detalles
  de implementación ni estado interno. **MSW** para mockear la red a nivel HTTP.
- **Playwright** para E2E de los flujos críticos.

## Accesibilidad y calidad
- HTML semántico primero (un button es un `<button>`, no un div con onClick). Labels en inputs,
  foco manejado en modales. Es criterio profesional, no un extra.";

/// Playbooks EMPOTRADOS de React: pasos A→B de las tareas que más se
/// repiten, para que dpx no explore a ciegas y aplique la convención correcta.
/// Se cargan cuando el focus activo es `react`. (nombre, cuándo, pasos).
#[allow(dead_code)]
pub const PLAYBOOKS: &[(&str, &str, &str)] = &[
    (
        "crear componente con estado",
        "USAR cuando: crear un componente React con estado, useState, useReducer, custom hook, o pantalla/feature nueva. NO para componentes puramente presentacionales (sin estado).",
        "1. Componente en `features/<feature>/components/` o `components/` con TypeScript estricto (nada de `any`).\n\
         2. Estado local con `useState`; si la lógica es compleja, extráela a un custom hook (`use<Algo>`) en `hooks/`.\n\
         3. Si hay varios estados relacionados, valora `useReducer` en vez de múltiples `useState`.\n\
         4. El estado derivado se CALCULA en el render (variable local), NUNCA lo dupliques en otro `useState` ni en un `useEffect`.\n\
         5. Props tipadas con interfaz explícita; `children` cuando toque. Keys estables en listas (id, nunca índice).\n\
         6. Verifica con `npm run dev` y los React DevTools (chequea re-renders y estado).",
    ),
    (
        "fetch de datos (TanStack Query)",
        "USAR cuando: llamar a una API, fetch, cargar datos del servidor, petición HTTP, GET/POST al backend, caché de datos. Palabras: API, fetch, axios, loading, error, datos remotos. NO para formularios ni para estado local del cliente.",
        "1. Server state ≠ client state: usa SIEMPRE TanStack Query (`@tanstack/react-query` v5+), NUNCA `useEffect` + `useState` artesanal.\n\
         2. `QueryClientProvider` en la raíz con `staleTime` y `retry` configurados.\n\
         3. `useQuery` para GET (cache automática, revalidación, estados loading/error). `useMutation` para POST/PUT/DELETE; `invalidateQueries` tras mutar.\n\
         4. Centraliza las funciones fetch en `api/` o `features/<feature>/api.ts` (con fetch o axios).\n\
         5. Estados de UI: `isLoading` → skeleton/spinner, `isError` → mensaje + retry, `data` → render normal.\n\
         6. Verifica con React DevTools (pestaña Query) y comprueba que la caché se invalida tras mutar.",
    ),
    (
        "formulario con validación",
        "USAR cuando: crear un formulario, validar campos, form, input, react-hook-form, zod, manejar submit, errores de validación. NO para un solo campo de búsqueda o toggle.",
        "1. Usa `react-hook-form` + `zod` (`@hookform/resolvers`). Define el schema Zod PRIMERO: nombre correcto, constraints reales.\n\
         2. `useForm<SchemaType>({ resolver: zodResolver(schema) })`, con `defaultValues` si aplica.\n\
         3. Registra campos con `{...register('campo')}`; errores con `formState.errors.campo?.message`.\n\
         4. `handleSubmit(onSubmit)` en el `<form>`. El onSubmit recibe datos YA validados y tipados.\n\
         5. Usa `isSubmitting` para deshabilitar el botón mientras se envía; muestra errores del servidor con `setError`.\n\
         6. Verifica: prueba el formulario manualmente (vacíos, formatos erróneos, submit exitoso).",
    ),
    (
        "test de componente (RTL + Vitest)",
        "USAR cuando: escribir o pedir un test, probar un componente, testing, Vitest, React Testing Library, RTL, MSW, mockear API. Palabras: test, probar, testing, Vitest, RTL.",
        "1. Usa Vitest + React Testing Library (tu `package.json` manda: si tiene Jest, sigue con Jest). Nada de Enzyme.\n\
         2. Testea COMPORTAMIENTO VISIBLE: `screen.getByRole`, `getByText`, `getByLabelText`. NUNCA testees estado interno ni implementación.\n\
         3. Mockea peticiones HTTP con MSW (`msw`): define handlers en `mocks/handlers.ts`, `setupServer` en setup de tests.\n\
         4. El test del camino feliz: renderiza, interactúa, verifica que aparece lo esperado. Un test por comportamiento, no por línea de código.\n\
         5. `userEvent` (no `fireEvent`) para simular interacciones reales. Usa `waitFor`/`findBy*` para asserts asíncronos.\n\
         6. Corre `npx vitest run` (o `npm test`) y reacciona a la salida real.",
    ),
    (
        "estructurar por features",
        "USAR cuando: organizar el proyecto, estructura de carpetas, mover archivos, refactorizar la estructura, crear una feature nueva. Palabras: feature, estructura, carpetas, organizar, refactorizar estructura.",
        "1. Organiza por FEATURE, no por tipo de archivo: `features/pedidos/{components,hooks,api,types}` mejor que un `components/` gigante.\n\
         2. Cada feature exporta su página/componente raíz; los detalles internos son privados de la feature.\n\
         3. Código COMPARTIDO va en `shared/` o `common/` (ui genérico: Button, Modal, Input). Nada de imports entre features (rompe acoplamiento).\n\
         4. Rutas con `React.lazy(() => import('./features/...'))` + `<Suspense fallback={...}>` para code splitting por feature.\n\
         5. Verifica: `npm run build` limpio y comprueba chunks separados en la salida (cada feature lazy → su propio .js).",
    ),
    (
        "corregir useEffect mal usado",
        "USAR cuando: el usuario tiene un useEffect sospechoso, quiere derivar estado, reaccionar a props, 'useEffect para actualizar estado', o hay un bug de re-renders. Palabras: useEffect, effect, dependencias, re-render, bucle infinito, sincronizar.",
        "1. PRIMERO: pregunta SIEMPRE '¿este efecto SINCRONIZA con algo externo (DOM, API del navegador, librería no-React)?'. Si la respuesta es no, el efecto sobra.\n\
         2. Patrón clásico a eliminar: `useEffect(() => setDerivado(props.x + props.y), [props.x, props.y])`. Se reemplaza por `const derivado = props.x + props.y` en el cuerpo del render.\n\
         3. Si el efecto dispara un fetch, reemplázalo por TanStack Query (ver playbook 'fetch de datos').\n\
         4. Si el efecto corre en cada render y actualiza estado, estás en un bucle: rompe la cadena (calcula en render o usa ref).\n\
         5. Cuándo SÍ usar useEffect: `addEventListener`/`removeEventListener`, timers (`setInterval`/`clearInterval`), WebSocket, integración con librerías externas no-React. SIEMPRE con cleanup.\n\
         6. Verifica: elimina el Strict Mode temporalmente para descartar doble-montaje y confirma que el comportamiento es el esperado.",
    ),
];
