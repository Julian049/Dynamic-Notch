# Dynamic Island para Windows

Un notch al estilo Apple pegado al borde superior de la pantalla, siempre encima de las demás ventanas. Crece al pasar el mouse.

## Funciones

- **Música**: título, artista y controles de lo que suene en Windows (Spotify, YouTube…), con letras sincronizadas de [lrclib.net](https://lrclib.net).
- **Claude Code** (opcional, en Ajustes): muestra si está pensando, qué herramienta usa, si espera permiso y cuándo termina. Funciona con Claude Code en Windows y en WSL.
- **Drop**: arrastra archivos a la isla y aparece un QR para descargarlos desde el celular, o subir archivos del celular al PC (`Descargas\Drop`). Solo red local; se apaga a los 10 minutos.
- **Auto-ocultar** e **Iniciar con Windows**, configurables en Ajustes (icono de la bandeja).

## Descargar

Desde [Releases](../../releases). El `.exe` no está firmado.

## Compilar

Hecho con [Tauri 2](https://tauri.app) (Rust + HTML/CSS/JS sin frameworks). Desde WSL, con cross-compile:

```sh
cd src-tauri
cargo xwin build --release --target x86_64-pc-windows-msvc
```

Con Rust para Windows instalado basta con `cargo build --release` en `src-tauri/`.

Pruebas: `node --test tests/lyrics.test.js` (JS) y `cargo xwin test --release --target x86_64-pc-windows-msvc` (Rust).

## Estructura

```
src-tauri/   app en Rust (ventana, bandeja, media, servidores de Claude y Drop)
  src/main.rs    configuración, visibilidad, comandos
  src/media.rs   sesión de medios de Windows
  src/claude.rs  hooks de Claude Code
  src/drop.rs    servidor de Drop y QR
ui/          interfaz (notch y ventana de Ajustes)
tests/       pruebas de JS
```

## Privacidad

Sin telemetría, cuentas ni auto-actualización. Lo único que sale del PC son las consultas de letras a lrclib.net.
