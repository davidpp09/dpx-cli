---
name: crear un skill / playbook para dpx
focus: dpx
cuando: "USAR cuando: crear, escribir o agregar un skill o playbook nuevo (curado o built-in de un stack: React, Python, Node, etc.), o cuando falte un playbook para una tarea que se repite."
---
Un skill es un PLAYBOOK A→B: le dice a dpx los pasos exactos de una tarea que se
repite, para que no explore a ciegas ni dé algo genérico. Hay dos tipos:

Crea `skills/<nombre-kebab>.md` con frontmatter + cuerpo:
```
---
name: <título corto y reconocible>
focus: <id del stack o "dpx">
cuando: "USAR cuando: <frases gatillo concretas>. NO usar para <contraejemplo>."
---
1. <paso A→B con la RUTA/función real>
2. ...
```

## Reglas de un BUEN skill (esto es lo que evita lo genérico)
- El `cuando` ES el gatillo: ponlo INSISTENTE, con frases y palabras reales que el
  usuario diría (dpx tiende a sub-disparar los skills). Incluye un "NO usar para…".
- Cuerpo CORTO y ESPECÍFICO: rutas, funciones, anotaciones, comandos REALES del
  stack — nada de "crea una clase y añade lógica". Si no es específico, no sirve.
- **Investiga antes de escribir** un stack que no domines: confirma las VERSIONES y
  convenciones ACTUALES (busca en la web si hace falta; alinéate con el bloque de
  versiones del focus pack). NUNCA inventes versiones ni APIs.
- Un solo playbook por tarea repetible; pasos numerados que terminen en "verifica".
