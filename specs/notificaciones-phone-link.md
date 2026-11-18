# Spec: notificaciones de Phone Link con su app real (icono y nombre)

Para: Sonnet (implementa). Revisa: Opus. Fecha: 2026-11-18.

## Problema

Una notificación de WhatsApp que llega por Phone Link aparece en la isla como "Phone Link", con el
icono de Phone Link (o la ficha con la inicial). Windows, en cambio, muestra el logo de WhatsApp.

Causa (verificada en vivo):

- `UserNotification.AppInfo()` devuelve `Microsoft.YourPhone_8wekyb3d8bbwe!App` ("Phone Link") para
  todas las apps del teléfono. Pero cada app tiene su propio AUMID en Windows:
  `Microsoft.YourPhone_8wekyb3d8bbwe!YourPhoneNotifications_com.whatsapp` (también existe como clave
  en `HKCU\...\Notifications\Settings`).
- El logo de WhatsApp va dentro del XML de la notificación como imagen `appLogoOverride`. La API
  `UserNotificationListener` no da acceso a las imágenes.
- Ambos datos están en la base de datos de notificaciones de Windows,
  `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db` (SQLite en modo WAL: hay
  `wpndatabase.db-wal` y `-shm` al lado, y las filas nuevas suelen estar en el WAL).

Lo que hay en la base para la notificación real (textos tapados con `…`):

```sql
SELECT h.PrimaryId, n.Payload
FROM Notification n JOIN NotificationHandler h ON n.HandlerId = h.RecordId
WHERE n.Id = 45518;
-- PrimaryId: Microsoft.YourPhone_8wekyb3d8bbwe!YourPhoneNotifications_com.whatsapp
```

```xml
<toast launch="…" hint-toastId="…" activationType="protocol"><visual><binding template="ToastGeneric"><image id="1" src="file:///C:\Users\Julian\AppData\Local\Packages\Microsoft.YourPhone_8wekyb3d8bbwe\TempState\app_icons\app_icon_com.whatsapp2.26.39.74.png" placement="appLogoOverride" hint-crop="square"/><text id="1">WhatsApp</text><text id="2">…remitente…</text><text id="3">…mensaje…</text></binding></visual><actions>…</actions></toast>
```

El PNG es legible desde un proceso normal de Windows (16 KB). `Payload` puede venir como BLOB o
como TEXT: leerlo como bytes y convertirlo con `from_utf8_lossy`.

## Paso 0: guardar lo que ya está hecho (antes de tocar nada)

En `main` hay arreglos sin commit (`src-tauri/src/main.rs`, `ui/main.js`): resize con un solo
`SetWindowPos`, ventana de ancho fijo y notificación que abre el notch. Crear la rama
`arreglos-0.8` desde `main` y hacer commit de esos dos archivos tal cual, con el mensaje
"Arreglos: ventana de ancho fijo, resize atómico, la notificación abre el notch". **No cambiarlos.**
Todo lo de esta spec va en un segundo commit en la misma rama. No hacer merge ni push.

## Qué hay que hacer (solo `src-tauri/src/notify.rs` y `Cargo.toml`)

1. **Dependencia:** `rusqlite` con la feature `winsqlite3`, que enlaza con el SQLite que trae
   Windows; `winsqlite3.lib` está en el SDK de cargo-xwin. Si no enlaza, usar `bundled` y anotarlo
   en el informe. No añadir ninguna otra dependencia (nada de regex ni parsers XML).

2. **`fn toast_source(id: u32) -> Option<(String, Option<Vec<u8>>)>`**: devuelve el AUMID real
   (`PrimaryId`) y los bytes del logo `appLogoOverride`, si existe.
   - Copiar `wpndatabase.db`, `wpndatabase.db-wal` y `wpndatabase.db-shm` a una carpeta propia,
     `%LOCALAPPDATA%\dev.julian.dynamicisland\wpn\`, abrir la copia en solo lectura y hacer la
     consulta de arriba con `WHERE n.Id = ?1`. No abrir la base original: el servicio de
     notificaciones la tiene abierta en modo WAL.
   - **Privacidad: borrar los tres archivos copiados nada más terminar la consulta, también si
     falla** (la copia contiene todas las notificaciones del usuario). Nunca registrar en el log
     ni guardar el payload.
   - Si cualquier paso falla, devolver `None`: la isla sigue como ahora.

3. **`fn logo_src(payload: &str) -> Option<String>`** (función pura, con prueba):
   - Busca la etiqueta `<image` que tenga `placement="appLogoOverride"` y devuelve su `src`.
   - Solo acepta `file:///` (quita ese prefijo y cambia `&amp;` por `&`); cualquier otro esquema
     (`ms-appx`, `http`…) da `None`.
   - Leer el archivo con el mismo límite de 16 MB que `media::read_stream`.

4. **En `run()`**, solo para la notificación que se va a emitir (no en cada sondeo):
   - Llamar a `toast_source(n.Id())`. Si devuelve un AUMID, usarlo en lugar del de `AppInfo`
     para todo: `notif_apps`, exclusiones, la caché de iconos y el campo `aumid` del evento.
   - **Phone Link:** si el AUMID contiene `!YourPhoneNotifications_`, el primer texto es el
     nombre de la app. En ese caso `app` = texto 1 ("WhatsApp"), `title` = texto 2, y `body` =
     el resto unido con espacios. En los demás casos, como ahora.
   - Icono: el logo `appLogoOverride` si existe; si no, el de `AppInfo`, como ahora. Se sigue
     cacheando por AUMID (ahora uno por app del teléfono, así que WhatsApp y Gmail no se pisan).
   - Así "WhatsApp" aparece en Ajustes como app propia y se puede excluir por separado. `apply()`
     ya la silencia en modo Notch porque su clave existe en el registro: no tocar `apply()`.

5. **Pruebas** (en `notify.rs`, módulo `tests`):
   - `logo_src` con el XML de ejemplo de arriba devuelve la ruta sin `file:///`. Sin imagen, o con
     `src="ms-appx:///..."`, devuelve `None`. Con `&amp;` en la ruta, la devuelve con `&`.
   - Separación de textos de Phone Link: si la extraes a una función pura, pruébala con
     `["WhatsApp", "+57 300", "hola"]`.

## Fuera de alcance

La UI (`ui/main.js` no necesita cambios: ya pinta `notifIcons[aumid]` y `n.app`), las animaciones,
los sonidos y el resto de la hoja de ruta. Si ves otro bug, anótalo en el informe; no lo arregles.

## Puertas (ejecutar todas y reportar el resultado de cada una en una línea)

Comandos según CLAUDE.md (compilar desde `src-tauri/` con
`CARGO_TARGET_DIR=~/.cache/dynamic-island-target`).

- **G1 Compila** — CHECK: `cargo xwin build --release --target x86_64-pc-windows-msvc` — EXPECT: `Finished`, sin warnings nuevos.
- **G2 Pruebas de Rust** — CHECK: `cargo xwin test --release --target x86_64-pc-windows-msvc --no-run`, copiar el `.exe` de pruebas a una carpeta de Windows y ejecutarlo — EXPECT: `test result: ok`, incluidas las pruebas nuevas.
- **G3 Prueba negativa** — CHECK: hacer que `logo_src` devuelva siempre `None`, compilar y ejecutar las pruebas, y luego restaurar y recompilar — EXPECT: la prueba de `logo_src` falla con el cambio y pasa al restaurar.
- **G4 Pruebas de JS** — CHECK: `node --test tests/*.js` — EXPECT: `fail 0`.
- **G5 En vivo, sin teléfono** — Cerrar la isla, lanzarla con CDP (ver CLAUDE.md) y mostrar un toast de PowerShell con logo:
  ```powershell
  [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null
  $png = "C:\Users\Julian\AppData\Local\Packages\Microsoft.YourPhone_8wekyb3d8bbwe\TempState\app_icons\app_icon_com.whatsapp2.26.39.74.png"
  $x = New-Object Windows.Data.Xml.Dom.XmlDocument
  $x.LoadXml("<toast><visual><binding template='ToastGeneric'><image placement='appLogoOverride' src='file:///$png'/><text>Prueba</text><text>Con logo</text></binding></visual></toast>")
  $aumid = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
  [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($aumid).Show([Windows.UI.Notifications.ToastNotification]::new($x))
  ```
  Desde CDP: `Object.keys(notifIcons)` y `notifIcons[aumid]` (no nulo), y una captura de la isla — EXPECT: el notch se abre y muestra el logo verde de WhatsApp en lugar de la ficha con la «W».
- **G6 Sin restos** — CHECK: `ls "/mnt/c/Users/Julian/AppData/Local/dev.julian.dynamicisland/wpn/"` después de G5 — EXPECT: vacía o inexistente.
- **G7 Al terminar** — cerrar la isla lanzada con CDP y dejarla abierta en modo normal.

Puertas manuales (las hace el usuario, las revisa Opus): un WhatsApp real por Phone Link abre el
notch con el logo de WhatsApp, "WhatsApp" como app, el remitente como título y el mensaje debajo, y
"WhatsApp" aparece en Ajustes como app aparte.

## Informe

Commits, una línea por puerta, decisiones que haya que revisar (por ejemplo, `bundled` en vez de
`winsqlite3`) y lo que quede pendiente. Actualizar en `CLAUDE.md` la línea de `notify.rs` en
"Arquitectura": se quita "Phone Link muestra su propio icono, no el de WhatsApp" y se explica la
lectura de `wpndatabase.db`.
