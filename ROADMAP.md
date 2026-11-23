# NEXØRA — Roadmap Oficial
> **"Nueva generación de optimización"**  
> **Plataforma:** Windows (Escritorio)  
> **Stack:** Tauri v2 (Rust + React + TypeScript)  
> **Estilo visual:** Futurista limpio con detalles en *pixel-art* detallado (retro-futurista técnico y sencillo, sin saturación cyberpunk).

---

## Principios de Diseño

1. **Seguridad primero:** Ningún archivo se elimina de forma permanente ni silenciosa. Todo requiere confirmación explícita del usuario y se envía a la **Papelera de reciclaje de Windows**.
2. **Liviana y bajo demanda:** Sin procesos residentes en segundo plano; solo se ejecuta cuando el usuario la abre.
3. **Precisión real en Windows:** Medir **tamaño real en disco** (*size on disk*) para evitar falsos positivos con archivos en la nube (OneDrive *Files On-Demand* que ocupan 0 bytes locales).
4. **Técnica con sencilleza:** Datos claros, rutas exactas y filtros potentes dentro de una interfaz amigable (*user-friendly*) con estética futurista y toques de *pixel-art* detallado.
5. **Permisos seguros:** Ejecución estándar como usuario normal, omitiendo de forma segura carpetas del sistema sin permisos.

---

## Fase 1 — MVP (v0.1 → v1.0): Visibilidad y Limpieza Segura

### 1.1 Motor de Escaneo (Rust / Core)
- [ ] Escaneo rápido multi-hilo de carpetas de usuario (`Descargas`, `Escritorio`, `Imágenes`, `Documentos`, `Videos`) y discos completos (`C:\`, `D:\`, unidades externas).
- [ ] Cálculo de **tamaño real en disco** (*size on disk* vía API de Windows) y detección de archivos *cloud-only* (OneDrive).
- [ ] Manejo seguro de errores de permisos (`Access Denied`), *symlinks/junctions* (evitar bucles infinitos) y rutas largas (`\\?\`).
- [ ] Progreso en tiempo real y cancelación inmediata de escaneos.
- [ ] Protección estricta de rutas críticas (`C:\Windows`, `Program Files`, `Program Files (x86)`, `ProgramData`).

### 1.2 Vista Combinada (UI — React + TypeScript)
- [ ] Identidad visual **NEXØRA**: tema futurista limpio con iconos/detalles en *pixel-art* detallado.
- [ ] **Dashboard:** Resumen por disco (usado/libre), tarjetas de "espacio recuperable" por categoría y accesos rápidos.
- [ ] **Explorador visual:** *Treemap* interactivo arriba + tabla técnica filtrable y ordenable abajo (tamaño real en disco, porcentaje, extensión, actividad y botón *"Abrir en el Explorador"*).
- [ ] **Centro de limpieza:** Pestañas dedicadas (*Instaladores*, *Capturas*, *Archivos grandes*, *Sin actividad*) con selección múltiple y cálculo en vivo del espacio a recuperar.

### 1.3 Detectores de "Ladrones de Espacio"
- [ ] **Instaladores:** `.exe`, `.msi`, `.msix`, `.iso`.
- [ ] **Capturas de pantalla:** Carpetas configurables por el usuario + autodetección por defecto en **español e inglés** (`Capturas de pantalla`, `Screenshots`, rutas de OneDrive y patrones `Captura de pantalla*` / `Screenshot*`).
- [ ] **Archivos grandes:** Umbral por defecto `> 500 MB` (configurable).
- [ ] **Archivos sin actividad reciente:** Umbral por defecto `> 6 meses` (configurable), presentado de forma *user-friendly* (usando `LastAccessTime` cuando NTFS lo reporta de forma fiable con respaldo automático en `LastWriteTime`, mostrando el detalle en tooltip).

### 1.4 Limpieza Segura
- [ ] Modal de confirmación explícita con lista de archivos, rutas y espacio total a liberar.
- [ ] Envío a la **Papelera de reciclaje de Windows** (crate nativo `trash` / API de Windows).
- [ ] Registro local (log) de limpiezas realizadas.

---

## Fase 2 — Inteligencia e Historial (v1.x)

### 2.1 Historial de Crecimiento
- [ ] Base de datos local ligera (SQLite) que guarda *snapshots* compactos tras cada escaneo.
- [ ] Gráficas de evolución: *"¿Qué carpetas crecieron más desde el último escaneo / en el último mes?"*.
- [ ] Indicadores visuales dentro de la app cuando una carpeta crece de forma inusual.

### 2.2 Limpieza Inteligente
- [ ] Detección de cachés seguras y temporales:
  - Temporales de Windows (`%TEMP%`).
  - Cachés de navegadores (con advertencia clara).
  - Carpetas pesadas de desarrollo (`node_modules`, `target`, `.venv`, caché de `npm`/`pip`/`cargo`, imágenes de Docker) señalando proyectos inactivos.
- [ ] **Lista Blanca (Exclusiones):** Marcar carpetas o archivos protegidos para que nunca se sugieran.

---

## Fase 3 — Automatización y Expansión (v2.x)

### 3.1 Limpieza Programada y Reglas
- [ ] Motor de reglas personalizadas por el usuario (ej. *"Marcar instaladores en Descargas con > 30 días"*).
- [ ] Integración opcional con el **Programador de tareas de Windows** (*Task Scheduler*) para escaneos periódicos sin dejar procesos residentes.
- [ ] Bandeja de revisión: al abrir NEXØRA, mostrar candidatos detectados esperando confirmación.

### 3.2 Actualizador de Herramientas (Dev & Sistema)
- [ ] Nueva sección dedicada para revisar y actualizar herramientas instaladas (ej. `git`, `node`, `rust`, `python`, utilidades comunes) integrándose con `winget` / gestores locales, siempre mostrando versión actual vs. disponible y pidiendo confirmación antes de actualizar.

### 3.3 Expansión Opcional
- [ ] Detector de archivos duplicados (tamaño + hash parcial/completo).
- [ ] Exportación de reportes (CSV / JSON).
- [ ] Empaquetado con instalador (`.msi` / `.exe`) y actualizaciones mediante GitHub Releases.
