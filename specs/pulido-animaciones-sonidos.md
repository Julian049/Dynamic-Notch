# 0.10.0: pulido, animaciones y sonidos

Plan aprobado por el usuario el 2026-11-18. Va en una sola versión (junta los puntos 3 y 4 de la hoja de ruta de CLAUDE.md). Rama `pulido`, que sale de `main`. Al terminar: versión 0.10.0 en `tauri.conf.json` y `Cargo.toml`, merge, tag y release con el `.exe`, siempre con el visto bueno del usuario.

Lo que pidió el usuario: animaciones *smooth* y minimalistas, y sonidos amigables en todo lo interactivo. Nada exagerado: ni cuentas atrás ni efectos largos. Las animaciones son "lo que lo es todo", sobre todo donde no hay sonido (silenciar, brillo).

## Reglas que no se negocian (ya están en CLAUDE.md)

- **Nada en bucle con CSS.** Cada fotograma recompone toda la ventana transparente de WebView2. Solo se permiten animaciones de **una sola vez** (keyframes de 150 a 600 ms que terminan solos). Lo que se repita va en `main.js` a unos 3 fps, como hoy la clase `blink` y el ecualizador.
- **Solo `transform` y `opacity`** en las animaciones nuevas, y `filter: blur()` solo si es breve. No animar `width`, `height` ni `top` de los hijos: la caja del notch ya anima con `--w`/`--h`.
- **La ventana tiene ancho fijo y solo cambia de alto.** `layout()` hace crecer la ventana *antes* de la animación y la encoge *después* (`shrinkTimer` con `ANIM_MS`). Si una animación sale fuera de la caja (rebote, temblor), tiene que caber en el margen `PAD`. Si no cabe, se anima el contenido dentro de `.shape`, que tiene `overflow:hidden`.
- **No redimensionar mientras un archivo está encima** (`dragArmed`/`dragging`).
- `prefers-reduced-motion` ya anula todas las animaciones (`style.css:248`). Debe seguir así.
- Tokens que ya existen: `--ease-out` (salida suave) y `--spring` (resorte con ~2 % de rebote). Usar esos, no inventar curvas nuevas.

## Animaciones

Implementación: clases que se ponen y se quitan desde `main.js`, con `@keyframes` de una sola pasada. Para relanzar una animación: quitar la clase, forzar un reflow (`el.offsetWidth`) y volver a ponerla. Otra opción es `el.animate()` (Web Animations API), que no deja clases colgadas y es la recomendada para los efectos puntuales.

### Generales del notch
| Momento | Animación |
|---|---|
| Aparecer (oculto → compacto) | La caja baja desde el borde: `translateY(-100%)` → 0 con `--spring` en 0,45 s, y el contenido entra con un fundido de 0,2 s y 0,1 s de retraso. Al ocultarse es al revés, con `--ease-out` en 0,25 s, sin rebote. |
| Expandir (hover) | Ya existe (resorte de 0,5 s). Pulido: el contenido de la vista entra con `opacity` 0→1 + `translateY(6px)`→0 + `scale(.98)`→1 en 0,25 s, con un retraso de 0,1 s. Los hijos principales (cabecera, cuerpo, controles) entran escalonados cada 30 ms. |
| Contraer | El contenido expandido se va primero (fundido de 0,12 s) y luego la caja se recoge. Hoy el contenido se queda visible mientras la caja se encoge y se recorta feo. |
| Cambiar de vista con el notch abierto (chip, llega una notificación…) | La vista saliente se funde en 0,12 s y la nueva entra como al expandir. La caja ajusta su alto con la transición que ya tiene. Nada de deslizar lateralmente. |
| Cambio en la línea compacta (canción nueva, otro estado) | El texto sale hacia arriba (`translateY(-40%)` + fundido, 0,15 s) y el nuevo entra desde abajo. El icono o la carátula del lado izquierdo hacen un fundido cruzado con un pequeño `scale(.85)`→1. |

### Música
| Momento | Animación |
|---|---|
| Play/pausa | El icono del botón hace un fundido cruzado con `scale(.7)`→1 (0,2 s, `--spring`). En compacto, el ecualizador baja sus barras a la altura mínima al pausar (eso ya lo hace el temporizador de JS) y la carátula baja a `opacity .6`. |
| Anterior/siguiente | El botón pulsado se hunde (`scale(.88)`, 0,1 s) y vuelve con el resorte. La carátula nueva entra con `translateX(±8px)` + fundido según la dirección. |
| Cambio de canción | Fundido cruzado de la carátula (0,3 s). El resplandor `.glow` cambia de color con `transition: background 0.6s`, que es de una sola vez y está permitido. |
| Letra | La línea actual sube: la anterior se funde hacia arriba, la nueva pasa de `opacity .5` a 1 con `translateY(4px)`→0 en 0,25 s. Una vez por línea, nunca en bucle. |
| Barra de progreso | Sin animación propia: la mueve el temporizador. Si salta más de 2 s (adelantar), `transition: transform .2s` una sola vez. |

### Sistema
| Momento | Animación |
|---|---|
| **Silenciar** | **Temblor corto** del icono del altavoz: `translateX` 0 → -2 → 2 → -1.5 → 1 → 0 px, en 0,32 s. El icono pasa a altavoz tachado: la raya se dibuja con `stroke-dashoffset` (0,2 s) o, si el icono es una máscara, con un fundido cruzado. El texto pasa de "Volumen · N %" a "Silenciado" con un fundido. Sin sonido (Windows ya está en mute). |
| **Quitar silencio** | El icono hace un pequeño "pop" (`scale` 0.8 → 1.08 → 1 con `--spring`, 0,3 s) y la raya se borra. Suena `sonido-activado.wav`. |
| Cambiar el volumen | Sin sonido. La barra se mueve con `transition: transform .12s var(--ease-out)` (si hoy usa `width`, pasar a `scaleX` con `transform-origin:left`). Al llegar a 0 % o al 100 %, un tope suave: `scaleX` 1 → 1.02 → 1, una sola vez por cada vez que se toca el tope. |
| **Brillo** | Sin sonido (decisión del usuario). La barra se mueve como la del volumen. El icono del sol gira según el nivel (`rotate(level * 0.9deg)`, `transition .2s`), y sus rayos crecen o encogen con `scale` según el nivel (0 % → .8, 100 % → 1.1). En los topes, el mismo rebote que el volumen. |
| Conectar el cargador | El rayo hace un pop (`scale` 0 → 1.1 → 1, `--spring`) y el relleno de la batería sube de 0 al nivel real en 0,5 s (`scaleX`). Suena `cargando.wav`. |
| Batería baja | El icono hace un pulso de color dos veces (rojo ↔ normal), 2 × 0,4 s, y se queda rojo. Suena `bateria-baja.wav`. |
| Bluetooth conectado | El icono del dispositivo entra con un pop y la batería se rellena como la del cargador. Suena `bluetooth-conectado.wav`. |
| Bluetooth desconectado | **Hoy no existe ese aviso** (system.rs solo emite `bt_connected`/`bt_low`). Añadir `bt_disconnected` en `Tracker` (con su prueba) y el texto "<nombre> desconectado". Animación: el icono se va a `opacity .4` con un `scale(.9)`. Suena `bluetooth-desconectado.wav`. |

### Notificaciones
| Momento | Animación |
|---|---|
| Llega una (abre el notch) | La caja se expande con el resorte y la tarjeta entra con `translateY(-8px)` → 0 + fundido. El icono de la app hace un pop pequeño con 60 ms de retraso. Suena `notificacion.wav`. |
| Llega otra con una ya en pantalla | La tarjeta actual baja un poco y se desvanece (`translateY(4px)`, `opacity 0`, 0,12 s) y la nueva entra desde arriba. El contador "N notificaciones" hace un pop. Suena otra vez, pero **como máximo una vez cada 1,5 s**: si llegan varias juntas, una sola. |
| Pendientes (vista de la campana) | Las filas entran escalonadas cada 40 ms con fundido y `translateY(4px)`. |

### Claude
| Momento | Animación |
|---|---|
| Empieza a trabajar | El punto de estado aparece con un pop. El parpadeo sigue siendo el de JS (`blink`), no CSS. |
| **Pide permiso** | La isla salta delante (eso ya pasa) con un **toque**: `translateY` 0 → 3 → 0 px, dos veces, en 0,4 s en total (como alguien que toca), sincronizado con el toc-toc-toc del sonido (los toques van a 0, 130 y 260 ms: hacer coincidir dos o tres). Después, el borde de atención con `blink` de JS. Suena `claude-permiso.wav`. |
| Terminó | El avatar hace un pop y aparece un check que se dibuja (`stroke-dashoffset`, 0,3 s). La tarjeta con el último párrafo entra con fundido. Suena `claude-termino.wav`, que reemplaza el `MessageBeep` de `claude.rs`. |

### Focus
| Momento | Animación |
|---|---|
| Iniciar | Los dígitos entran con fundido y `scale(.96)`→1. La barra se llena desde 0 con `scaleX`. Suena `focus-inicio.wav`. |
| Pasa a descanso | Los dígitos hacen un fundido cruzado (salen hacia arriba y entran desde abajo, como la línea compacta). El acento de color pasa al del descanso con `transition: color .4s`. Suena `focus-descanso.wav`. |
| Termina el focus | El icono de la luna hace un pop y la isla vuelve a su estado normal. Suena `focus-fin.wav`. |
| Cada segundo del temporizador | **Sin animación** (sería un bucle). Solo cambia el texto. |
| Pausar/terminar | El botón se hunde, como los de música. |

### Drop
| Momento | Animación |
|---|---|
| Arrastrar encima | La zona punteada entra con `scale(.97)`→1 + fundido. Al entrar el archivo en la zona, el borde pasa a color de acento (una sola transición). Prohibido redimensionar: la ventana ya es grande. |
| QR listo | El QR aparece con `scale(.92)`→1 + fundido, y sus módulos con una "cortina" de arriba abajo (`clip-path: inset(0 0 100% 0)` → `inset(0)`, 0,35 s). Suena `drop-listo.wav`. |
| Llega un archivo del teléfono | Aparece "Recibido · nombre" con un pop y un check que se dibuja. Suena `archivo-recibido.wav`. |
| Cerrar (X) | La X gira 90° mientras se pulsa y el QR se va con `scale(.95)` + fundido. Suena `clic.wav`. |
| Cuenta atrás "Disponible m:ss" | Sin animación. El punto verde ya es estático. |

### Botones e interacción
- Todos los botones del notch (play/pausa, anterior/siguiente, pastillas de focus, chips, X): presionados con `transform: scale(.92)` en `:active` (0,1 s) y al soltar vuelven con `--spring`. Ya hay una transición de `transform 0.12s` en `style.css:222`: unificarla allí. Suena `clic.wav`.
- Chips de modo: el seleccionado desliza su fondo hasta el chip nuevo (un pseudo-elemento con `translateX`, 0,25 s, `--spring`).
- Hover sobre botones: solo cambio de fondo (ya existe). Sin sonido al pasar el mouse.

## Sonidos

Archivos aprobados en `sounds/` (los genera `sounds/gen.py`, sin archivos de terceros):
`notificacion`, `claude-permiso`, `claude-termino`, `focus-inicio`, `focus-descanso`, `focus-fin`, `bateria-baja`, `cargando`, `bluetooth-conectado`, `bluetooth-desconectado`, `drop-listo`, `archivo-recibido`, `abrir`, `cerrar`, `sonido-activado`, `clic`.

- **Dónde:** copiar los `.wav` a `ui/sounds/` (van embebidos en el `.exe`). `gen.py` se queda en `sounds/`, fuera de `ui/`, para que no se embeba, y debe escribir directamente en `ui/sounds/` (cambiar `OUT`). Después se borran los `.wav` de `sounds/`.
- **Cómo:** reproducirlos en `main.js` con Web Audio: precargar cada archivo con `fetch` + `decodeAudioData` en un `AudioContext` y, para tocar uno, crear un `AudioBufferSourceNode` → `GainNode` → destino. Es más fiable y barato que crear un `new Audio()` por cada sonido. Ojo: un `AudioContext` puede empezar suspendido sin un gesto del usuario. Probar si WebView2 de Tauri lo deja sonar sin interacción. Si no, `resume()` en el primer `mouseenter`, y si ni así, la opción B: reproducir desde Rust con `PlaySoundW` + `SND_MEMORY | SND_ASYNC` con los bytes en `include_bytes!`. Comprobarlo antes de construir todo lo demás.
- **Cuándo suena cada uno:** el que indican las tablas. Además: `abrir` al expandir el notch **solo** por hover o clic del usuario (no cuando lo abre una notificación, que ya tiene su sonido) y `cerrar` al contraerlo por salir el mouse. Volumen: `sonido-activado` solo al pasar de muted a no muted, nunca en cada cambio de nivel. Brillo: nunca.
- **Callados:**
  - En focus (concentración), igual que hoy la isla se calla: solo suena `claude-permiso` y los de fin de fase.
  - Una notificación de una app excluida no suena (ya no llega).
  - Límite general: el mismo sonido no se repite antes de 1,5 s.
- **Ajustes:**
  - Un interruptor nuevo "Sonidos de la isla" (activado por defecto) en General, con la clave `sounds` en `Config` (main.rs) siguiendo el patrón de `sound`.
  - Debajo, "Sonidos de interacción" (`abrir`, `cerrar` y `clic`), con la clave `ui_sounds`, activado por defecto y que depende del anterior (`data-needs="sounds"`).
  - El "Sonido al terminar" de Claude que ya existe (`sound`) se mantiene y ahora toca `claude-termino.wav` desde la UI. Quitar `beep()` de `claude.rs` y su llamada (`claude.rs:244` y `:279`), o dejar que la UI decida.
- **Volumen:** sin control propio, sigue el volumen de Windows. Ganancia fija: los archivos ya vienen normalizados bajos (pico de -9 dBFS; los de interacción, más bajos).

## Orden de trabajo sugerido

1. Comprobar que suena audio desde la WebView sin interacción (o pasar a la opción B). Montar `play(name)` y el interruptor de Ajustes.
2. Conectar los sonidos a cada evento (lo más rápido de verificar con eventos simulados por CDP).
3. Animaciones generales (aparecer, expandir, contraer, cambiar de vista), que son las que más se notan.
4. Las de cada vista, en el orden de las tablas.
5. `bt_disconnected` en Rust, con su prueba.
6. Medir el consumo como dice CLAUDE.md (con música, isla visible): no debe pasar del ~3 % de un núcleo en reposo. Las animaciones de una sola vez no deberían cambiarlo.

## Cómo verificar

- Por CDP (CLAUDE.md, "Depurar la UI"), emitir `media`, `notification`, `system` (`{kind:'volume', level, muted}`, `{kind:'brightness', level}`, `charging`, `battery_low`…), `drop` y los hooks de Claude con `curl`, y hacer capturas a mitad de animación (`screenshot` justo después del `emit`) para ver que ocurre.
- El usuario no puede ver las animaciones desde aquí y no quiere artifacts (tokens). Dejar la versión nueva abierta para que él la pruebe en vivo.
- Pruebas: las de Rust (incluida la nueva de `bt_disconnected`) y `node --test tests/lyrics.test.js`.
