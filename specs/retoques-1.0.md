# Retoques estéticos antes de la 1.0

Decidido con el usuario el 2026-11-18. **Solo estética y presentación**: sin funciones nuevas fuera de las de abajo. Rama: seguir en `pulido`. La base es el Figma `NB3A7p0YxNE0OKa07QeUXD`, página "Dynamic Notch" (frame de componentes `5:227` y lámina "Dynamic Notch — Estados" `7:2`). Leer cada nodo con `get_design_context` (medidas, colores, tipografía) y no inventar valores.

Reglas que siguen valiendo (ver `specs/pulido-animaciones-sonidos.md` y CLAUDE.md):

- Nada se mueve en bucle con CSS (consumo). Solo animaciones de una sola vez con `fx()` / `@keyframes` de una pasada.
- Sin `scrollHeight` para medir; la ventana es de ancho fijo `MAX_W` (si algún ancho nuevo supera el actual, `MAX_W` lo recoge solo).
- Los sonidos ya están; no se añaden ni cambian.
- No tocar el `settings.json` de la raíz.

Orden de trabajo: 1 → 2 → 3 → 4 → 5. Cada punto es un commit.

---

## 1. Claude terminó: aviso → último mensaje → "Listo" pasivo

Figma: aviso `55:614` (324 × 154), último mensaje `5:120` (ya existe), pasivo `55:593` (346 × 34). Lámina: fila `7:144` y `55:1620`. Movimiento: nota `55:1767`.

Flujo:

1. Llega `Stop` → el notch **se abre solo** (como la notificación, `setExpanded(true)`), con la vista nueva `claude-alert`: mascota grande centrada con chispas y check verde, "Tarea terminada" y `proyecto · 4 min 12 s`.
2. A los **1,5 s** la misma forma se transforma, sin cerrarse, en la vista actual "Claude terminó" (último mensaje). Cross-fade de 160 ms.
3. A los **6 s** sin mouse encima se recoge → compacto **"Listo" pasivo**: mascota, check verde, "Listo", proyecto y duración (`4 min`).
4. Hover sobre "Listo" vuelve a abrir el último mensaje. "Listo" desaparece con el tiempo de "Ocultar automáticamente" o al haberse visto (comportamiento actual de `forgetSeenSessions`).

Animación (una sola vez, nota `55:1767`): apertura con resorte (rebote 0.25, 0.4 s); la mascota da un salto de 4 px; las chispas aparecen y se apagan en 400 ms; el check entra con escala 0.6 → 1. El sonido `claude-termino` sigue igual, al abrir.

- Respetar `cfg.claude_done`. Durante focus `work` **no se abre sola** (la isla se calla): solo queda "Listo" en compacto, sin sonido, como hoy.
- Si el mouse está encima cuando llega, salta directo al paso 2.
- Si llega otra cosa más urgente (permiso, notificación), manda la otra; el "Listo" queda guardado.

## 2. Claude pide permiso: aviso → pendiente con brillo rojo

Figma: aviso `57:696` (324 × 190), pendiente `57:733` (406 × 34). Lámina: `57:1510` y `57:1561`. Movimiento: nota `57:1757`.

Flujo:

1. Llega el permiso → el notch se abre solo con la vista `claude-alert` en modo permiso: mascota con "!" rojo y brillo rojo alrededor, "Necesita tu permiso", `proyecto · esperando 0:12` y una pastilla "Quiere ejecutar `npm test`" (el comando sale de `detail`/`tool_input`; si no hay, se omite la pastilla).
2. A los **2,5 s** sin mouse se recoge → compacto **pendiente**: mascota, "!" rojo, "Esperando permiso", proyecto y tiempo esperando en rojo, con el notch rodeado de **brillo rojo**.
3. No se oculta hasta que se responda en la terminal (como hoy). Hover abre la vista de Claude actual.

Animación de entrada (una sola vez): resorte con rebote 0.3 (0.35 s), un sacudón lateral de ±3 px (sustituye al "toc-toc" vertical actual) y el brillo rojo destella 0 → 100 % → 60 %. Sonido `claude-permiso` igual.

**Decisión de consumo:** el Figma pide que el brillo "respire" 40 → 70 % cada 2 s en pasivo. Eso es un bucle y va contra la regla. Se hace **fijo al 60 %** (lo que el Figma ya define para "reduce motion"). Si el usuario lo quiere vivo, la alternativa barata es que `main.js` cambie la opacidad en saltos con el mismo reloj de `blink` existente, sin transición; se mide antes de aceptarlo.

- El brillo rojo también se mantiene con varias sesiones si alguna espera (lámina `24:499`).
- Se abre sola también durante focus (el permiso es la excepción, como hoy).

## 3. Baterías: pasivas (a la vista) y pestaña Batería, equipo y Bluetooth por separado

Figma: compacto solo baterías `60:715` (198 × 34), con música `60:750` (419 × 34), con Claude `60:774` (327 × 34), pestaña portátil `59:1689` (404 × 258), pestaña escritorio `59:1765` (404 × 184). Lámina: sección `60:1917`. Movimiento: nota `60:2752`.

### Ajustes (Elementos → Sistema), todo apagado por defecto

- **"Batería del equipo a la vista"**: muestra el % del portátil en el compacto. Deshabilitado con aviso si el equipo no tiene batería (`batteries.pc == null`).
- **"Baterías Bluetooth a la vista"**: muestra los dispositivos Bluetooth conectados con su % en el compacto.

Son interruptores independientes (el usuario quiere poder ver solo uno). Config nueva en `main.rs` (`glance_pc`, `glance_bt`, por defecto `false`), igual que los demás interruptores.

### Compacto (pasivo)

- Las baterías se suman **al final** de cualquier estado pasivo (música, Claude trabajando, Listo…), separadas por una `divider`. La letra se acorta para hacerles sitio (el ancho del compacto crece lo que midan, como hoy hace la campana con `v.w += …`).
- Formato: icono pequeño + `78%`. Verde = cargando, rojo = batería baja (≤ 20 %). Portátil primero, luego los dispositivos.
- Con otra actividad: portátil + **un** dispositivo (el primero de la lista) para no saturar; el resto se ve en la pestaña. Solas (sin otra actividad): todos.
- **No aparecen** durante avisos (volumen, brillo, notificación, permiso, aviso de Claude): ahí manda el aviso.
- **Sin nada más que mostrar:** con algún interruptor encendido, en lugar de ocultarse del todo la isla queda en el compacto de baterías (`60:715`). Requiere que la UI lo diga con `set_keep` (como hace con la música). Si los dos están apagados, se oculta como hoy.

Animación (nota `60:2752`): entran por la derecha con fade de 160 ms al pasar a pasivo; no se animan al cambiar de evento (quietas como ancla); al cambiar un % solo cross-fade del número.

### Pestaña Batería (expandido)

- Nuevo modo en los chips (`MODES` / `CHIP_ICON`): icono de batería, visible cuando hay batería del equipo o algún dispositivo.
- La vista `batteries` actual se rehace como en el Figma, **en dos bloques separados**:
  - "Este equipo": fila "Portátil" con "Cargando · lleno en 40 min" / "En batería".
  - "Dispositivos Bluetooth · valores aproximados": una fila por dispositivo, "Conectados" / "Batería baja" en rojo.
- PC de mesa (sin batería): solo el bloque Bluetooth, título "Dispositivos" (`59:1765`).
- Sin dispositivos: solo el bloque del equipo.
- La pestaña aparece aunque los interruptores "a la vista" estén apagados (eso solo controla el compacto).

## 4. Hora en el compacto

No está en el Figma; diseño nuestro, con el mismo estilo que las baterías a la vista.

- Ajuste **"Hora a la vista"** (Elementos → Sistema), apagado por defecto (`glance_clock` en config).
- En el compacto pasivo, en el mismo grupo de las baterías y **antes** de ellas: `14:32` con la clase `val` (formato 24 h o el del sistema vía `toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'})`).
- Se actualiza una vez por minuto con un `setTimeout` alineado al cambio de minuto (no un intervalo de 1 s). Parado con la isla oculta, como el ecualizador.
- Como las baterías: cuenta para "quedarse visible en vez de ocultarse" y no aparece durante avisos.

## 5. Proporción del notch compacto (más corto y más alto)

Pedido del usuario: hoy se ve "flaquito y largo" (música 380 × 34, Claude 440 × 34, campana +60; proporción ~11:1). Quiere algo más parecido al notch de una MacBook (~6:1), sin que la letra deje de leerse.

- Probar **dos variantes** y que el usuario elija viéndolas en la isla real con su música:
  - A: alto 38, música 300, Claude 360.
  - B: alto 40, música 280, Claude 330.
- Escalar en proporción los demás anchos del compacto (`compactView`: 300/320/330/340…) y los `+60` de la campana y lo que sumen las baterías/hora.
- `COMPACT_H.classic` cambia; `win11` sube lo mismo. Revisar que carátula (26 px), iconos y texto quedan centrados verticalmente.
- La marquesina de la letra aparecerá más a menudo: es lo esperado.
- Este punto va **el último**, porque cambia los anchos que fijan los puntos 3 y 4.

---

## Verificación (la hago yo por CDP al terminar)

1. **Terminó:** emitir `Stop` simulado (curl al hook o `emit('claude')`). Se abre sola, aviso con check, a los 1,5 s pasa al mensaje, a los 6 s queda "Listo" compacto; hover reabre el mensaje. En focus `work`: no se abre ni suena.
2. **Permiso:** emitir `Notification` con `permission_prompt`. Se abre sola con brillo rojo y sacudón lateral, a los 2,5 s queda pendiente con brillo fijo y tiempo en rojo; no se oculta; al llegar `PostToolUse` desaparece el brillo.
3. **Baterías:** con cada interruptor por separado, con música, con Claude, solas, durante un aviso (no salen) y con la isla "sin nada" (queda el compacto de baterías en vez de ocultarse). Pestaña Batería con y sin `pc`, con y sin dispositivos.
4. **Hora:** aparece, cambia al pasar el minuto, no sale en avisos.
5. **Proporción:** capturas de las dos variantes para que el usuario elija.
6. **Consumo:** nada nuevo animándose en bucle (Performance de CDP en reposo con permiso pendiente y baterías a la vista: sin fotogramas continuos).
7. Pruebas de Rust y JS siguen verdes; compilar y dejar la isla corriendo con los ajustes del usuario como estaban.
