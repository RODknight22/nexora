# AGENTS.md — Guía de Trabajo para Agentes IA (NEXØRA)

Este archivo define las reglas, principios y flujo de trabajo obligatorios para cualquier agente de IA que trabaje en **NEXØRA** (*"Nueva generación de optimización"*).

---

## 1. Contexto del Proyecto
- **Producto:** **NEXØRA**, app de escritorio liviana para Windows que analiza el uso real del disco, detecta archivos que consumen espacio (instaladores, capturas, archivos grandes y archivos sin actividad) y permite limpiarlos de forma segura.
- **Stack tecnológico:** **Tauri v2** (Backend en **Rust**, Frontend en **React + TypeScript**).
- **Estilo visual:** Futurista limpio con detalles en *pixel-art* detallado (técnico y sencillo, sin recargar).
- **Documento de visión y fases:** Ver [ROADMAP.md](./ROADMAP.md).

---

## 2. Reglas de Seguridad (Innegociables)
Al escribir o modificar lógica que interactúe con el sistema de archivos:
1. **NUNCA eliminar archivos permanentemente por defecto.** Toda acción de borrado debe enviar los archivos a la **Papelera de reciclaje de Windows**.
2. **SIEMPRE requerir confirmación explícita del usuario** en la interfaz antes de mover archivos a la Papelera.
3. **Proteger rutas críticas del sistema:** Jamás sugerir ni permitir borrar contenido dentro de `C:\Windows`, `C:\Program Files`, `C:\Program Files (x86)`, `C:\ProgramData` ni la raíz del sistema.
4. **Tamaño real en disco:** Usar el tamaño real ocupado en disco (*size on disk*) en Windows para no reportar archivos *cloud-only* (OneDrive) como consumidores locales.
5. **Permisos estándar:** La app corre como usuario normal; manejar `Access Denied`, *symlinks* / *junctions* (evitar bucles infinitos) y rutas largas (`\\?\`) sin crashear.
6. **Pruebas seguras:** Los tests de escaneo o borrado deben ejecutarse **exclusivamente en directorios temporales aislados** creados durante el test y eliminados al terminar.

---

## 3. Autoría y Privacidad en Git (Innegociable)
- Todos los commits, ramas, PRs e Issues deben quedar **exclusivamente a nombre del propietario del repositorio** (usando su `git config user.name` y `user.email`).
- **PROHIBIDO** agregar `Co-Authored-By`, trailers de IA, firmas de herramientas o cualquier mención a agentes/IA en mensajes de commit, descripciones de PR, comentarios de código o documentación del producto.

---

## 4. Flujo de Trabajo con Git y GitHub (GitHub Flow)

### Ciclo por Tarea (1 Issue = 1 Rama = 1 PR)
1. **Mini-plan previo:** Antes de crear código o abrir un PR, presentar un mini-plan breve al usuario y esperar su aprobación.
2. **Issue:** Cada tarea o historia corresponde a un **GitHub Issue**.
3. **Rama:** Crear una rama desde `main` actualizada con prefijos semánticos:
   - `feat/<descripcion-corta>` — Nueva funcionalidad.
   - `fix/<descripcion-corta>` — Corrección de errores.
   - `docs/<descripcion-corta>` — Documentación.
   - `refactor/<descripcion-corta>` — Refactorización sin cambio funcional.
   - `test/<descripcion-corta>` — Pruebas.
   - `chore/<descripcion-corta>` — Configuración, CI, dependencias.
   - **Prohibido hacer commits o push directo a `main`.**
4. **Commits (Conventional Commits):**
   - `feat(scanner): calcular tamaño real en disco en Windows`
   - `fix(cleaner): manejar rutas Unicode al enviar a papelera`
   - `chore(ci): agregar workflow de lint y tests en GitHub Actions`
5. **Pull Request (PR):**
   - Abrir el PR hacia `main` usando `gh pr create`, enlazando el Issue (`Closes #<num>`).
   - Incluir: resumen de cambios, cómo se probó y verificación de seguridad si toca el sistema de archivos.
   - Debe pasar **CI en GitHub Actions** (lint, compilación y tests) antes del merge.
   - **El usuario realiza la revisión y el merge final en GitHub.**

### Trabajo en Paralelo
- Si trabajan varios agentes simultáneamente, cada uno debe operar en su propia rama (idealmente en un `git worktree` aislado) y en módulos distintos para evitar conflictos.

---

## 5. Calidad de Código y Arquitectura
- **Separación estricta:**
  - `src-tauri/` (Rust Core): Escaneo, cálculo de tamaño real, detectores, historial y envío a Papelera (sin acoplar lógica de negocio al frontend).
  - `src/` (React + TypeScript): Vistas, estado visual, *Treemap*, tablas técnicas y confirmaciones.
- **Comandos de verificación** *(se activarán al inicializar el proyecto Tauri v2)*:
  - Frontend: `npm run lint`, `npm run build`, `npm test`
  - Backend Rust: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`

---

## 6. Alcance
- No implementar funcionalidades de fases futuras del [ROADMAP.md](./ROADMAP.md) sin aprobación explícita.
- Ante cualquier duda sobre UX o seguridad al borrar archivos, preguntar antes de asumir.
