# NEXØRA

> **Nueva generación de optimización**

Aplicación de escritorio liviana para Windows enfocada en analizar el uso real del disco, identificar archivos que consumen espacio innecesario y limpiarlos de forma **100% segura** (siempre con confirmación previa y envío a la Papelera de reciclaje de Windows).

## Características principales (Fase 1 — MVP)
- **Escaneo rápido y preciso:** Carpetas de usuario y discos completos midiendo el **tamaño real en disco** (*size on disk*), evitando falsos positivos con archivos en la nube (OneDrive).
- **Vista combinada:** Dashboard resumen + mapa visual (*Treemap*) + tabla técnica filtrable y ordenable.
- **Detectores inteligentes:**
  - Instaladores (`.exe`, `.msi`, `.msix`, `.iso`).
  - Capturas de pantalla (carpetas configurables con autodetección en español e inglés).
  - Archivos grandes (`> 500 MB`, configurable).
  - Archivos sin actividad reciente (`> 6 meses`, configurable).
- **Limpieza segura:** Confirmación explícita antes de mover cualquier archivo a la Papelera de reciclaje y protección estricta de rutas críticas del sistema.

## Stack tecnológico
- **Tauri v2**
- **Backend:** Rust
- **Frontend:** React + TypeScript

## Hoja de ruta
Consulta el plan completo por fases en [ROADMAP.md](./ROADMAP.md).
