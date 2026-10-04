# Rosadeck Launcher Reboot — biblioteca primero (F7)

Fecha: 2026-10-02. `cargo test --workspace`: **294 passed, 0 failed,
0 warnings**. Sin HDMI conectado, pero con **lanzamientos reales supervisados**
de las cuatro plataformas (ver "v8").

## Filosofía nueva (Decision, reemplaza F5-CLI)

1. La biblioteca es el centro, no los flags: escanear → identificar → jugar.
2. Convención sobre configuración: `~/roms/<platform>/`, títulos No-Intro,
   artwork same-stem, stats locales sin cuentas ni red.
3. Pantalla completa siempre al lanzar (flag verificado por adaptador).
4. Fallos honestos: binario ausente, preset ausente, plataforma sin adaptador.

## game-library (nuevo)

`platform` (5 plataformas, extensiones curadas, emulador por defecto),
`title` (parser No-Intro + artículo inicial, verificado con la colección
real), `scan` (ids estables SHA-256, sidecars omitidos), `art` (same-stem +
`art/<platform>/`, placeholder si falta), `stats` (favoritos, tiempo jugado,
atómico tmp+rename). En vivo: 13 juegos detectados, títulos limpios.

## Fullscreen verificado (Observed)

- Dolphin: `-C Dolphin.Display.Fullscreen=True` (docs upstream) +
  `-C GFX.Settings.VSync=True` (clave observada en tu `GFX.ini` real).
- Azahar: `-f` + ruta posicional, `-w` si no (observado en `--help` real).
  Nuevo adaptador (3DS jugable: Mario Kart 7, etc.).
- RetroArch: `-f` (documentado upstream, sin binario aquí: no validado).
- Dolphin rechaza `.zip` (assumption documentada a re-verificar).

## Rediseño de la interfaz (v2, Observed)

La biblioteca pasó de lista de texto a **estantería de carátulas**:

- `cover.rs`: decodifica PNG/JPEG reales (`image` 0.25) y los pinta con
  bloques medios `▀` en truecolor (2 píxeles verticales por celda), con
  runs SGR fusionados por color. Sin protocolo gráfico de kitty: funciona en
  cualquier terminal truecolor. Caché memoizada por (ruta, w, h).
- Sin carátula → **carátula generada**: degradado + banda + iniciales del
  título en bitmap 5x7 escalado (`Luigi's Mansion` → `LM`), determinista por
  hash del título, con marco en el color de la plataforma.
- `layout.rs`: fuerza bruta sobre nº de filas posibles y **maximiza el área
  de carátula** que aún cabe Showing `count` juegos; la etiqueta siempre
  ≥18 columnas (carátula centrada sobre ella, estilo estantería).
- `theme.rs`: profundidad de color (truecolor / xterm-256 / plano) desde
  `COLORTERM`/`TERM`/`NO_COLOR`; `--no-color` y `--ascii` para terminales
  tontos; cuantización RGB→256 con cubo 6x6x6 + rampa de grises.
- Navegación 2-D (↑↓←→, PageUp/Down, Home/End) sobre el grid; chips de
  plataforma con contadores en vivo; detalle con tamaño legible,
  emulador y ruta; leyenda de teclas.
- `tui-frame` ahora mide **ancho visible ignorando escapes ANSI**
  (`visible_len`), trunca sin romper secuencias y cierra el estilo con reset.

Invariantes cubiertos por tests: cada línea mide exactamente
`Layout::drawn_width` columnas visibles en 7 tamaños de terminal (con y sin
color), nunca excede el ancho del terminal, y un PNG magenta real en disco
llega al frame como `38;2;255;0;255` (E2E). En vivo: 190x46 → 7 columnas,
`bare_LF=0`, frame de 58 KB en ~5 ms (con caché; sin decodificar por frame).

`image` es la única dependencia nueva (0.25, features png+jpeg).

## CLI

`library [--platform/--query/--json]`, `play <query> [--id/--dry-run/
--json/--yes]` (fuzzy único o candidatos, exit 11 si ambiguo/ausente).
`play` graba stats solo en lanzamiento real exitoso.

## Demos en vivo (Observed)

`library` (13 juegos), `play Luigi's Mansion --dry-run` (Dolphin
fullscreen+VSync, VALID), `play Mario Kart 7 --dry-run` (azahar `-f`,
VALID). `monitors -j` intacto; sin estado display escrito.

## v8: "solo falta que se lanzen los juegos, y el de N64 no tenemos emulador" (Observed)

Petición del usuario. Resultado: los cuatro emuladores lanzan de verdad, y el
N64 sí tenía emulador — estaba instalado y sin cablear.

### Dos adaptadores nuevos (flags verificados en esta máquina)

| Plataforma | Adaptador | Comando | Cómo se verificó el flag |
| --- | --- | --- | --- |
| SNES | `snes9x` | `snes9x-gtk <rom>` + config propia (ver v10) | `--help` **abre la GUI** en vez de imprimir; `--fullscreen` se parsea pero **no hace nada** (medido en v10) |
| N64 | `mupen64plus` | `mupen64plus --fullscreen <rom>` | `mupen64plus --help` (2.6.0, `extra`) documenta `--fullscreen`, `--nospeedlimit`, `--resolution`, `--gfx`, `--audio`, `--rsp` y ROM **posicional**; ventana medida a pantalla completa |

`pacman -Qs` ya decía `extra/mupen64plus 2.6.0-1 [instalado]`: el binario es
`mupen64plus` (UI de consola completa, ventana propia), no
`mupen64plus-fceux` como se buscaba antes. Ninguno de los dos necesita shaders
slangp: `prepare` **rechaza** un preset en vez de ignorarlo.

RetroArch deja de ser el emulador por defecto de SNES y N64 (no está instalado
y su perfil apunta a un core inexistente `/cores/snes9x_libretro.so`): queda
como perfil opt-in. `Platform::default_emulator` = lo que de verdad se puede
ejecutar.

### El bloqueo real: la máquina de estados no permitía lanzar sin display

`session.step(SessionState::Launching)` fallaba con
`illegal session transition: Preparing -> Launching`: la única ruta legal era
`Preparing → DisplayApplying → DisplayVerified → Launching`. Como el camino de
la biblioteca es justamente *sin* cambio de display (y sin HDMI no hay plan que
aplicar), **ningún juego podía lanzarse nunca**: `rosadeck: session`, exit 2.
Añadida la transición `Preparing → Launching` con test propio; la ruta con
display sigue exigiendo verificación (`al_display_verified_no_se_puede_saltar`).

### Binario ausente = error nombrado, no nota

`prepare` resolvía el binario y, si no lo encontraba, **usaba el nombre tal cual
y añadía una nota**; `--dry-run` decía `VALID` para un programa inexistente y
el fallo sólo aparecía al hacer `exec`. Ahora `resolve_binary(...)?` →
`EmulatorError::BinaryNotFound`, que sale como exit 17 con tres líneas honestas:

```
rosadeck: PREPARE_FAILED: emulator binary not found: dolphin-emu-not-installed
rosadeck: install it yourself with: pacman -S dolphin-emu
rosadeck: check with: rosadeck emulators
```

Rosadeck **no instala nada**: la pista es texto para copiar.

### La biblioteca dice la verdad antes de Enter

- `emulators.rs` (nuevo, puro): lee `~/.config/rosadeck/emulators/*.toml` y
  resuelve el binario (misma `resolve_in_path` que el CLI). Tres estados
  distinguibles: `dolphin ✓`, `retroarch NOT INSTALLED · pacman -S retroarch`,
  `azahar NOT CONFIGURED` (sin perfil). Estilo `Alert` nuevo (rojo, con rol
  pywal legible ≥4.5:1).
- La leyenda pasa de `⏎ play` a `⏎ needs emulator` cuando el juego seleccionado
  no se puede lanzar.
- Enter hace **preflight**: `rosadeck play --id <id> --dry-run` (sin spawn, sin
  mutación). Si falla, la línea de estado muestra el stderr del CLI + la pista
  del paquete y **no** se sale de la pantalla alternativa. La decisión de qué
  emulador usar sigue siendo del CLI: la biblioteca no reimplementa política.
- `rosadeck library` imprime `emulator=dolphin ok` / `MISSING`, y `--json`
  añade `emulator_ready`. `rosadeck emulators` lista plataformas por perfil e
  `install` cuando falta.

### Bug encontrado de paso: la línea de estado nunca se pintaba

`app.status` se asignaba en diez sitios (`played X`, `favoritado X`,
`rescanned: N games`, `palette: …`) y **no se leía en ningún sitio**: el campo
no existía en `FrameInput`. Todos los mensajes eran invisibles, incluidos los
nuevos errores de lanzamiento. Ahora tiene fila propia sobre la regla del pie
(`footer = 6`), siempre pintada (aunque vacía) para que la geometría no se
mueva, con test que fija posición y ancho.

### `rosadeck … | head` ya no entra en pánico

Rust ignora SIGPIPE, así que cerrar la tubería mataba el proceso con
`failed printing to stdout: Broken pipe`. `restore_default_sigpipe()` pone la
disposición por defecto: exit 141 silencioso, como cualquier herramienta Unix.

### Lanzamientos reales supervisados (Observed)

`tools/pty_launch_check.py <comm> [teclas]`: abre la biblioteca real en un pty
de 190x46, pulsa Enter y comprueba que el proceso del emulador aparece **con el
ROM en su línea de órdenes**. Las cuatro plataformas:

```
OK  /usr/sbin/dolphin-emu -b -C Dolphin.Display.Fullscreen=True -C GFX.Settings.VSync=True -e .../Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz
OK  /usr/sbin/azahar -f .../Mario Kart 7 (USA) (En,Fr,Es) (Rev 1).cci
OK  /usr/sbin/snes9x-gtk --fullscreen .../Super Mario All-Stars (Europe).sfc
OK  /usr/sbin/mupen64plus --fullscreen ".../Mario Party 2 (Europe) (En,Fr,De,Es,It).z64"
```

Con `HOME` falso y el binario de Dolphin renombrado, el mismo script falla como
debe y el navegador muestra
`rosadeck: PREPARE_FAILED: emulator binary not found: dolphin-emu-not-installed · pacman -S dolphin-emu`
sin lanzar nada. Detalle del script: `pgrep -x <nombre>` (nombre de proceso,
nunca línea de órdenes) — un `pkill -f` con el nombre del emulador mataba
también a la shell que ejecutaba el test.

`play --id <id-inexistente>` devolvía exit 11 **sin un solo mensaje**; ahora
dice cuántos ROMs escaneó y cómo listarlos.

## v9: "al lanzar juegos de dolphin la ui se buggea al cerrar el emulador" (Observed)

### Causa raíz 1: el pintor creía que el frame anterior seguía en pantalla

Al volver del emulador la biblioteca hace `EnterAlternateScreen` — un búfer
**nuevo y vacío** — pero **no** invalidaba su modelo de pantalla. El pintor
trabaja por diferencias: `Screen::draw` sólo reescribe las filas cuyo texto
cambió (status, detalle, contador de plays). Todas las demás quedaban con lo
que el emulador había dejado escrito:

```
antes |  ↑↓←→ move  ⏎ play  f fav  / find  …
ahora |rosadeck: emulator exited -1 (crash or error); display restored,It).rvzttings.VSync=True -e /hom
```

Fijado con `screen.invalidate()` justo después de `EnterAlternateScreen`, igual
que ya se hacía en el *resize* y en la recarga de pywal. Además se envía DECSTR
(`\x1b[!p`) + `\x1b[0m` antes de reentrar: Dolphin es una app Qt y deja estado
(SGR, cursor) que no se puede suponer.

### Causa raíz 2: el emulador escribía en la terminal del navegador

`--quiet` (nuevo) manda stdout/stderr del emulador a
`$XDG_STATE_HOME/rosadeck/emulators.log` y su stdin a `/dev/null`
(`QuietSpawner` en `emulator-core`). El emulador dibuja en su propia ventana
X11/Wayland, así que su consola es sólo un log; lo que se evitaba es esto:

```
A signal was received. A second signal will force Dolphin to stop.
```

escrito **mientras el navegador esperaba**, y el volcado de argv de Qt al
morir. `QuietSpawner` cae al terminal heredado si el log no se puede abrir
(la partida arranca igual) y tiene dos tests: el stdout/stderr acaba en el
fichero, y un log inabrible no impide lanzar. La biblioteca lo pasa siempre; una
persona en su shell no lo usa y sigue viendo la salida del emulador.

### Dos mentiras más, por el mismo camino

- `rosadeck play` decía `display restored` aunque no se hubiera tocado
  ninguna pantalla (el camino de la biblioteca no aplica plan). Ahora:
  `display restored` sólo si hubo snapshot, y `display untouched` si no.
- El navegador traducía cualquier salida no cero como `did not start`. El
  código 19 significa "el emulador **sí** corrió y terminó con error": cerrar
  un juego no es un fallo. Ahora: `X closed (exit 19)`.

### El test que lo atrapa (Observed)

`tools/pty_return_check.py <emulator-comm> [teclas]`: biblioteca real en un pty
de 190x46 → Enter → esperar el emulador → **cerrarlo como un jugador** →
comparar dos **modelos de terminal** (`tools/vtterm.py`, celda a celda, no
bytes): el frame antes de lanzar contra el frame al volver,Se permite que
cambien sólo los chips, la línea de estado y las tres filas de detalle (los plays
suben de verdad); cabecera, carrusel, regla y leyenda deben volver idénticas.

```
OK  cabecera, carrusel, regla y leyenda idénticos; cambiaron [40] (estado/detalle/chips, esperado)
```

Comprobado con dientes: quitando `screen.invalidate()` el mismo check falla en
la fila de la leyenda.

Dos trampas del propio test, documentadas porque costaron tiempo:

1. Dolphin **ignora el primer SIGTERM** ("A second signal will force Dolphin to
   stop"): el emulador no moría, el navegador no volvía nunca, y una versión
   anterior del check «pasaba» sin comprobar nada (replay desde el `?1049h`
   inicial = frame entero). Ahora escala a SIGKILL y exige >2 KB de repintado
   tras la vuelta.
2. Un `pkill -f dolphin` mata también a la shell que ejecuta el test. Sólo
   `pgrep -x <nombre>` y `kill` por PID.

## v10: "snes9x-gtk no abre los juegos en pantalla completa" (Observed)

Petición del usuario. La causa fue una **afirmación falsa propia**: en v8 se
documentó que `--fullscreen` estaba "verificado" en `snes9x-gtk`. Lo que se
verificó entonces fue sólo que el flag **se parseaba**, que no es lo mismo.

### Lo que se midió (Observed, `hyprctl clients -j`, sólo lectura)

| Invocación | Ventana | `fullscreen` |
| --- | --- | --- |
| `snes9x-gtk <rom>` | 634x676 | 0 |
| `snes9x-gtk --fullscreen <rom>` | 634x676 | **0** (igual que sin flag) |
| `XDG_CONFIG_HOME=<nuestro> snes9x-gtk <rom>` | 1280x720 en (0,0) | **2** |

El fullscreen de Snes9x es un **ajuste de configuración**, no un flag:
`~/.config/snes9x/snes9x.conf` tiene `[Window State] Fullscreen` y
`[Display] FullscreenOnOpen` (las dos mueven el comportamiento: la primera es el
estado guardado, la segunda la casilla "Use fullscreen on ROM open"). Y
`-conf <fichero>` se parsea pero **se ignora**: el fichero nunca se abre.

### El arreglo: copia parcheada, nunca el config del usuario

`prepare` describe el lanzamiento (no escribe nada, se puede inspeccionar con
`--dry-run`), así que el fichero se materializa en el último momento posible:

- nuevo gancho `Emulator::finalize(&mut PreparedLaunch)` en `emulator-core`,
  llamado por el CLI **justo antes de spawnear** (nunca en `--dry-run`),
- el adaptador de Snes9x copia `~/.config/snes9x/snes9x.conf` (o
  `$XDG_CONFIG_HOME/snes9x/snes9x.conf`), fuerza `Fullscreen = true` y
  `FullscreenOnOpen = true` conservando **la alineación y los comentarios** del
  usuario, y escribe la copia en
  `$XDG_STATE_HOME/rosadeck/snes9x-config/snes9x/snes9x.conf`,
- el hijo se lanza con `XDG_CONFIG_HOME` apuntando a ese directorio.

Se copia el config del usuario y no se escribe uno mínimo a propósito: así sus
mandos, filtros y sonido siguen valiendo y sólo cambian las dos claves de
fullscreen. El original se **lee, nunca se escribe** (md5 comprobado antes y
después). Detalles honestos: la copia se regenera en cada lanzamiento (Snes9x
la reescribe al salir, y da igual porque partimos siempre del original), y si
el destino no se puede escribir `finalize` falla con exit 17 en vez de abrir un
juego en ventana sin avisar.

### Falsa alarma ajena: Dolphin sí abre a pantalla completa

El nuevo `tools/pty_fullscreen_check.py` ("¿la ventana del emulador está
fullscreen?") dio `fullscreen: 0` para Dolphin y casi se documenta como bug. No
lo era: Dolphin tiene **varias ventanas** —`org.kde.dolphin` (la principal),
un popup de carga y `dolphin-emu` (la del juego)— y el check cogía la primera.
Con la ventana correcta: `dolphin-emu [1280, 720] en (0,0), fullscreen=2`, o
sea `-C Dolphin.Display.Fullscreen=True` sí funciona (clave confirmada en el
upstream: `MAIN_FULLSCREEN = Info<bool>{System::Main, "Display", "Fullscreen"}`).
El tool ahora prefiere una ventana fullscreen y, si no hay, la mayor.

Estado real de las cuatro plataformas, todas launching desde la biblioteca y
midiendo la ventana:

```
snes9x-gtk     OK  la ventana está en pantalla completa
mupen64plus    OK  la ventana está en pantalla completa
azahar         OK  la ventana está en pantalla completa
dolphin-emu    OK  la ventana está en pantalla completa
```

## v11: "las imágenes jpg no tienen soporte y hay imágenes que no muestra" (Observed)

Dos quejas del usuario. Una era un bug de verdad y la otra, casi: los JPG **sí**
funcionaban, pero la interfaz no lo decía, y el flujo tenía un agujero.

### Las portadas de Zelda no se encontraban (bug)

`Legend of Zelda, The - A Link to the Past.png` no casaba con nada:

- el ROM se llama `Legend of Zelda, The - A Link to the Past (Europe).sfc`,
- el título parseado (y el que se guarda en la biblioteca) es
  `The Legend of Zelda - A Link to the Past`,
- el candidato que se generaba era ese título limpio, y el *stem* completo.

Falta una tercera forma: la del propio ROM, `X, The - Subtítulo`. Nuevo
`title::trailed_article()` — el inverso exacto de `parse_title` — y
`candidate_names` lo añade como candidato más:

```
The Legend of Zelda - A Link Between Worlds  ->  Legend of Zelda, The - A Link Between Worlds
The Last Story                                ->  Last Story, The
Luigi's Mansion                               ->  (nada: no hay artículo)
```

Test de regresión con los dos juegos reales (SNES y 3DS, png y jpg).

### Los JPG sí: pero la nota mentía y había que pulsar `r`

Comprobado con los cuatro JPG de la colección (600x900, baseline YCbCr):
`image` 0.25 trae `jpeg` por defecto y decodifican; el `cover` del panel de
detalle ya decía `cover Mario Party 2.jpg`. Aun así el flujo era malo:

1. **La nota preguntaba por existencia, no por decodificación**: cualquier
   fichero con nombre válido se anunciaba como portada aunque el decodificador
   lo rechazase. Ahora `cover_note` decodifica al tamaño del carrusel (memo hit,
   no trabajo extra) y dice `unreadable` en rojo si no puede. Test con un
   `Broken Cover.jpg` que no es una imagen.
2. **Una portada nueva sólo aparecía con `r`**: `art_path` memoiza el resultado
   para siempre, así que dejar un fichero en `~/roms/covers/` con el navegador
   abierto no se veía (da igual que sea PNG o JPG: el usuario lo lee como
   "las imágenes jpg no tienen soporte"). Ahora se vigila una **huella**: mtime de
   cada directorio candidato + mtime/tamaño de cada portada en uso. Al cambiar,
   el navegador invalida caché, capa de imágenes y pantalla, y lo dice
   (`artwork updated`).
   - Se corrigió un falso positivo de la primera versión: el sello incluía las
     portadas *ya resueltas*, que crecen al mover el carrusel, así que saltaba
     "artwork updated" solo al arrancar. Ahora el conjunto vigilado se calcula
     una vez por reescaneo (`art_files`), no por sondeo.

Verificado en vivo (pty, sin pulsar ninguna tecla):

```
estado inicial : ... cover generated
tras crear Game.jpg (sin teclas):
    artwork updated
    ... cover Game.jpg unreadable          <- el fichero de la prueba no es una imagen
```

Con las 12 portadas reales, todas pintadas y ninguna "unreadable":

```
Luigi's Mansion.png · Mario Kart 7.png · Mario Party 2.jpg · Mario Party 7.png
Mario Party 9.jpg · New Super Mario Bros. 2.png · New Super Mario Bros. Wii.jpg
Super Mario 3D Land.png · Super Mario All-Stars.png · Super Mario Sunshine.jpg
Legend of Zelda, The - A Link Between Worlds.png · Legend of Zelda, The - A Link to the Past.png
```

`rosadeck art`: 12/12 con portada (antes 10/12).

## v12: "las portadas de X, Y, Z no aparecen" (Observed)

La queja 돌아щала a seis juegos: Luigi's Mansion, Mario Kart 7, Mario Party 2,
Mario Party 9, New Super Mario Bros. Wii y Super Mario Sunshine. Los cuatro JPEG
de la colección estaban **todos** en la lista. Dos bugs de la capa de imágenes,
ambos invisibles desde el pty (sin kitty nadie ve la diferencia).

### Bug A: se mandaba el fichero tal cual diciendo que era PNG

`transmit` escribía los **bytes crudos** del fichero con `f=100`. En kitty `f=100`
significa "el payload es un PNG"; un JPEG enviado así lo rechaza. Y como todo va
con `q=2` (para que el terminal no conteste por stdin), **el error no se ve
nunca**: la portada sencillamente no aparece. Por eso los JPEG "no tenían
soporte", y por eso mis comprobaciones anteriores en pty no lo detectaron — un
pty sin kitty se traga la respuesta que aquí se pierde.

Ahora `read()` garantiza PNG: si el fichero ya es PNG se manda tal cual (sin
re-codificar), y cualquier otro formato se decodifica y se re-emite como PNG.

```
antes:  _Ga=t,f=100,q=2,i=7,z=-1,…;<bytes JPEG ÿØ…>
ahora:  _Ga=t,f=100,q=2,i=7,z=0,…;<bytes PNG…>
```

### Bug B: las imágenes iban *debajo* del texto

`z=-1` significa "dibujada bajo el texto". El hueco de la portada lo emite la
capa de texto como espacios (`cell()` deja el interior vacío), así que **cualquier
repintado de esas filas —mover la selección, cambiar la línea de estado, un
resize— las tapaba**. De ahí el patrón cambiante de "algunas sí, otras no": las
que se pintaron antes de perder su fila. Con `z=0` la portada va **encima** del
texto y no hay forma de que un espacio la borre; el hueco es vacío por
construcción, así que nada queda tapado (el borde, el título y la línea de
metadatos viven fuera del área de la portada).

Tests: PNG de entrada pasa intacto byte a byte; un BMP se re-emite como PNG con
los píxeles intactos; basura → no se manda nada; y los bytes del **cable**, no
sólo del encoder, empiezan por la firma PNG.

### Bug C: las portadas se colocaban *antes* del `Clear(All)`

El que quedaba: la primera ventana (Luigi's Mansion, Mario Kart 7, Mario Party
2) nunca se veía, y las demás sí. El orden en el bucle era:

```
?1049h ?25l ?2026h → a=t i=1 → 7;23H → a=p i=1 → … → ?2026l
                    ?2026h → [2J  ← borra también las colocaciones
```

Un repintado completo empieza por `2J`, y **kitty descarta las colocaciones de
imagen al limpiar la pantalla**. Como las portadas se enviaban primero, el
propio frame las borraba. Al hacer scroll no hay `2J`, por eso el resto de
ventanas sí se veían: el fallo era siempre "la ventana que acaba de pintarse
completa" — el arranque, un resize, un cambio de tema, un cambio de portada o
la vuelta de un emulador.

Arreglo: **texto primero, portadas después**, dentro de un único bloque
sincronizado (antes eran dos bloques):

```rust
if text_dirty { screen.draw(&mut out, &frame.text, app.w, app.h, false) }
if image_work { layer.write(&mut out, &diff); layer.commit(&diff, &written) }
```

Dos detalles: `Screen::draw` recibe `sync=false` porque el bloque lo abre el
llamante, y el bloque **no** se abre si no hay nada que pintar (si no, cada
sondeo de 400 ms emitía un bloque sincronizado vacío).

### Verificación (Observed)

`tools/wire_graphics_check.py`: recorre la biblioteca real en un pty de 190x46
pulsando flecha por flecha, y sobre los **bytes** comprueba dos cosas — que el
primer chunk de cada transmisión empieza por la firma PNG, y que ningún
`Clear(All)` aparezca después de una colocación dentro del mismo bloque:

```
bytes=12783867 bloques=12
chunks a=t=3084 no-PNG=0
colocaciones a=p=30
¿algún Clear(All) borra portadas ya colocadas en el mismo bloque? no
```

Con el orden viejo el mismo check falla; con el nuevo pasa. El resto de la
batería sigue verde: `pty_return_check.py` (vuelta del emulador),
`pty_frame_check.py` (sin escalones en raw mode), `vt_compare.py` (repintado
parcial == completo) y `pty_fullscreen_check.py` (las cuatro plataformas a
pantalla completa).

Nota: `pty_frame_check.py` forzaba el render de bloques porque, con la capa de
imágenes, 2 MB de kitty graphics enterran el frame en la captura; ahora lo dice
en el docstring y el peso de cada capa se comprueba con su propio tool.

Nota de método: varios fixtures de `images.rs` escribían `b"x"` como "imagen" y
funcionaban sólo porque los bytes se mandaban sin mirar. Con la conversión a PNG
esos fixtures son incorrectos de verdad: ahora escriben PNG reales (y uno
ruidoso, para que siga habiéndose payloads de varios chunks).

## v13: rediseño del selector — el centro encima, los lados detrás (Observed)

Petición: el carrusel es de una fila y tres columnas; que **el juego de enmedio
sea la capa sobrepuesta** y los de alado queden **por debajo, con desvanecimiento
y blur**.

### Geometría (`layout.rs`)

- `window_for(cursor, count)` ahora **centra** la selección: `cursor - cols/2`
  acotado a la última página. Antes la ventana sólo se deslizaba lo justo, así
  que el juego elegido podía estar en el borde.
- `side_inset()` = `cover_h / 12` (acotado 1..4) filas en blanco arriba y abajo
  de la portada de los vecinos, y `side_cover_h()` = `cover_h - 2 * inset`: la
  celda conserva su altura pero el arte se ve más pequeño, y eso es lo que hace
  que el centro domine sin cambiar la retícula.
- `selected_col()` es el único sitio que decide qué celda es la capa superior.

### Tratamiento de portada (`cover.rs`)

`Treatment::{Crisp, Recessed}` + `recede(img, backdrop)`:

1. **blur** de caja con radio `ancho/40` acotado a 1..6 (relativo al tamaño, así
   que se ve igual en bloques y a resolución nativa),
2. **desaturación** al 30% del croma — el croma es lo que el ojo lee como "en
   foco",
3. **mezcla al 40% con el fondo** de la paleta: en tema oscuro se oscurece, en
   tema claro se aclara, en vez de oscurecer siempre.

El tratamiento se aplica **después** del reescalado en el render de bloques (no
cuesta nada) y **antes** de codificar en el de imágenes, con el mismo código: una
portada se retira igual por los dos caminos. Las portadas *generadas* también se
retiran, o la estantería se rompería en los juegos sin arte.

La clave de caché incluye tratamiento y fondo: una portada retraída hacia una
paleta distinta es otra imagen, y una recarga de pywal no puede servir la vieja.

### Capas, no sólo colores

- Con el protocolo gráfico, el centro se coloca con `z=1` y los vecinos con
  `z=0`: el de enmedio es la capa sobrepuesta **en la capa de imágenes**, no
  sólo en el texto.
- El hueco de la portada lo emite el texto como espacios y por eso se coloca
  **encima** del texto (`z>=0`, v12): un repintado nunca puede taparla.

### Bug del pintor que el rediseño destapó

`row_patch` tenía dos casos: "cambió el texto" (parchea las columnas con
carácter distinto) y "mismo texto, distinto estilo". Ganaba el primero, así que
un cambio de estilo **fuera** del rango de caracteres se quedaba sin pintar.

Hasta ahora no se notaba porque una fila o tenía arte o tenía estilo. Al mover
la selección pasa lo contrario: en la misma fila el artwork de la celda nueva
aparece y el borde de la vieja se apaga. Resultado: el borde se quedaba en el
color anterior.

Reescrito como una sola pasada que compara **carácter y estilo por columna**
(`cells_of` resuelve el estilo vigente en cada columna, así que dos columnas sólo
son iguales si se ven igual):

```rust
for ((oc, och, ostyle), (nc, nch, nstyle)) in old_cells.iter().zip(new_cells.iter()) {
    if och != nch || ostyle != nstyle { first_col.get_or_insert(*oc); last_col = last_col.max(*oc); }
}
```

Test de regresión que aplica el parche a la fila vieja y compara con la nueva en
un **modelo de terminal** (caracteres y estilos), no en bytes.

### Verificación (Observed)

`cargo test --workspace`: 295 tests. Los del frame comprueban que la celda
seleccionada tiene arte en la primera fila de la banda y que las laterales
empiezan `inset` filas más abajo, y que la misma portada leída con los dos
tratamientos tiene croma claramente menor detrás.

Batería externa, toda en verde: `pty_frame_check` (sin escalones, `bare_LF=0`),
`vt_compare` (repintado parcial == completo), `pty_return_check` (vuelta del
emulador) y `wire_graphics_check` (PNG en el cable, sin `Clear(All)` que borre
colocaciones).

## v14: el selector como carrusel de verdad, con el juego enfocado encima (Observed)

Petición: "tipo vista de carrusel con enfoque en el juego que se está
seleccionando". El v13 sólo resaltaba el centro de una fila plana; esto es un
**abanico de cartas** con profundidad.

### `layout::fan` — el plan del abanico (`layout.rs`)

Aritmética pura, sin dibujo:

- la carta enfocada usa `cover_w` x `cover_h` completos, sin recorte;
- cada vecino `k` se estrecha `0.66 * 0.72^(k-1)` y se acorta `0.9 * 0.86^(k-1)`,
  así que la profundidad se nota en el tamaño además del color;
- `hidden = 45%` de su ancho: **la parte interior queda tapada** por la carta
  siguiente, que es lo que produce el solapamiento del carrusel;
- se añade una carta por lado y por iteración (simetría) mientras quepa, con
  tope `MAX_FAN = 7`.

Tres bugs que sólo aparecieron al escribir los tests:

1. el mínimo por carta (`MIN_CARD_TOTAL`) **frenaba el encogimiento**: a partir
   de la segunda distancia todas las tarjetas salían iguales y el "carrusel" era
   una fila de muñones. Ahora el suelo es 5 columnas totales y el encogimiento
   nunca se detiene;
2. el bucle comprobaba el hueco **una vez por iteración pero añadía dos cartas**:
   el banda se salía del terminal (46 columnas en un hueco de 30). Ahora se
   comprueba el par;
3. `fan_offset` centraba contra el abanico del peor caso (`MAX_FAN`) mientras el
   frame dibujaba el real: con pocos juegos la estantería salía descentrada.
   Ahora recibe las cartas reales.

También desapareció el concepto de "ventana deslizante": el abanico se construye
desde el cursor, sin `window_for`.

### La tarjeta se recorta, no se estira

`Crop::{left,right,full}` + `Cover::crop`:

- **bloques**: la portada se carga al ancho completo de la carta y se recorta el
  borde interior, así que conserva su escala (si se escalara al ancho visible,
  el arte encogería dos veces);
- **imágenes**: kitty recorta con el rectángulo de origen (`x`/`w`), medido con
  `image::image_dimensions` (sólo cabecera) → la portada llega a resolución
  nativa y recortada, no estirada.

Las portadas generadas pasan por el mismo camino (`cover_cropped` + generación
completa y recorte), o la estantería se rompería en los juegos sin arte.

### Capas

El foco se coloca con `z=1` y los vecinos con `z=0`, y en el texto se pinta
**último**: por eso queda encima en las dos capas.

### Composición por filas

Cada tarjeta responde con exactamente sus columnas en cada fila de la banda y
con blancos en las filas que su arte más corto no alcanza; no hay aritmética de
solapamiento en el ensamblado (que es donde antes se colaron filas de ancho
incorrecto).

Tests nuevos: simetría izquierda/derecha, encogimiento monótono, recorte no
nulo, el abanico cabe en el ancho disponible en 5 tamaños x 4 números de juegos,
centrado con diferencia ≤1 columna, y el test del frame comprueba con un modelo
de terminal que la fila enfocada tiene arte en la primera fila y el vecino
empieza más abajo, más estrecho y desaturado.

### Verificación (Observed)

`cargo test --workspace`: **294 passed, 0 failed, 0 warnings**.

Batería externa en verde: `pty_frame_check` (`bare_LF=0`), `vt_compare`
(repintado parcial == completo), `wire_graphics_check` (PNG en el cable, sin
`Clear(All)` que borre colocaciones) y `pty_return_check` (vuelta del emulador).

## v15: "la UI está buggeada y todo se siente lento" (Observed)

Dos quejas a la vez. Ninguna era subjectiva: las dos se reproducen midiendo.
Salieron **cinco** bugs de colocación de portadas (todos del mismo bloque: el
abanico se calculaba en dos sitios distintos que se desincronizaron al usar el
inset) y **un** cuello de botella de 2,4 s por portada.

### Los cinco bugs de colocación

1. **`Crop::hidden()` devolvía las columnas *conservadas*.** Sin argumentos no
   puede saber cuántas se ocultan, así que devolvía las que kept: un vecino
   pedía `full - kept` celdas y recortaba `kept` píxeles — visible y oculto
   intercambiados. El plan se calculaba mal en `plan_for_cropped`, y
   `source_rect` recortaba la imagen por el lado equivocado. **Borrado**: ahora
   el plan calcula `cols = crop.kept()` y `hidden = card_w - cols` en el único
   sitio que sabe las tres cosas. Test nuevo: `a_neighbours_plan_crops_the_inner_edge_and_keeps_its_visible_columns`
   (comprueba columnas, píxeles de origen y lado exterior en los dos sentidos).
2. **La banda se centraba con dos offsets distintos.** El texto empezaba en
   `layout.margin`; las colocaciones de imagen en
   `margin + (available - fan_width(plan))/2`. Con el plan completo (7 cartas)
   la diferencia es de ~30 columnas: **las portadas flotaban lejos de su
   tarjeta**, que es exactamente lo que se veía. Ahora los dos usan
   `layout::band_offset(margin, available, drawn)`, y se centra sobre las
   cartas *realmente dibujadas* (en los extremos de la biblioteca el abanico
   es más estrecho).
3. **La fila de arte no rellenaba con espacios.** `format!("{v}{body}{v}")`
   sin relleno: en modo bloques `body` nunca está vacío y se notaba poco, pero
   **con imágenes el interior es vacío** y la banda entera colapsaba a 3
   columnas mientras las portadas seguían donde decía el plan.
4. **Las columnas de la tarjeta eran 1-based y el resto del frame 0-based.**
   kitty coloca en la posición del *cursor* (`x`,`y` en minúsculas son el
   rectángulo origen en píxeles; verificado contra la spec, no de memoria), así
   que el arte va en `x + 2` (borde + arte + base 1), no en `x + 1`: cada
   portada se dibujaba encima de su propio borde y se quedaba una columna
   corta.
5. **El `inset` no se aplicaba al texto, solo a las imágenes.** Y al aplicarlo,
   las filas *por encima* de una tarjeta dejaban de reservar sus columnas: cada
   fila desplazaba la banda de forma distinta (con 7 cartas, 18 columnas de
   deriva). Ahora `band_row < inset` pinta el ancho en blanco y el esqueleto es
   idéntico en todas las filas.

Tests que fijan el conjunto: `image_plans_tile_the_band_without_overlapping`
(comprueba, para cada plan y para **cada fila de su tarjeta**, que el borde
izquierdo, el interior en blanco y el borde derecho están donde el plan dice;
y que el interior queda en blanco donde irá la imagen). Comprobado con
dientes: revirtiendo el punto 5 el test falla con `the card of Plan {...} is
not framed on row 5`.

### El cuello de botella: 2,4 s por portada

`recede` aplicaba un desenfoque de radio `w/40`: en un fichero de 600x900 son
15 px, un box blur de 31x31 sobre 540.000 píxeles = **2,43 s**, y el abanico
manda seis. Ahora el radio es absoluto y pequeño (`clamp(w/24, 1, 3)`) y, para
transmitir, la imagen **se reduce antes** (`fit_for_transmit`, lado ≤ 200 px,
Nearest porque va a ser desenfocada y reescalada por el terminal):
**2,43 s → 65 ms** y el payload de 630 KiB → 31 KiB.

Encima: un maestro decodificado por `(fichero, tamaño)` compartido por todos
los tratamientos y recortes, y los vecinos se derivan del maestro mayor
(`biggest`) en vez de redecodificar el fichero. Y la nota de portada del panel
de detalle ya **no pide la portada reescalada** para saber si decodifica:
`CoverCache::decodes()` memoiza el veredicto por fichero (150 ms por tecla
justo para escribir un nombre).

### Medición (Observed)

Todo con `tools/` en pty 190x46, 12 juegos con carátula real, compilando
**release**:

| | antes (debug) | ahora (release) |
|---|---|---|
| primer frame pintado | 659 ms | **96 ms** |
| flecha: primer byte | 136-436 ms | **13-33 ms** |
| flecha: frame pintado | 316-460 ms | **38-56 ms** |
| portada vecina (blur+codificación) | 2,43 s | 65 ms |
| bytes por flecha | 61 KiB | 31-62 KiB |

Aviso honesto: buena parte de la lentitud que se veía era el **binario de
debug**. `cargo run` / `target/debug` queda en 300-500 ms por tecla aunque el
código sea el mismo; la cifra de arriba es de `target/release`.

Herramienta nueva: `wire_graphics_check.py` reproduce además el texto de cada
bloque en un modelo de terminal (`tools/vtterm.py`), sigue el cursor (que es
donde kitty coloca) y exige que cada colocación caiga en el interior en blanco
de su tarjeta, con bordes `│` a ambos lados. Resultado actual: **72 de 72
colocaciones correctas** (antes: 12 de 72).

`cargo test --workspace`: **296 passed, 0 failed, 0 warnings**.

## v16: el efecto del carrusel no estaba logrado (Observed)

Con la banda ya alineada (v15) el abanico *tampoco* parecía un carrusel. Mirando
el frame renderizado, la causa era de dibujo, no de geometría: **cada tarjeta
era una caja completa** (`│` arte `│` + una columna de aire), así que entre
tarjetas se veían `│ │` — dos barras y un hueco. Siete cajas de tamaños
distintos se leen como siete miniaturas, no como una estantería con una tarjeta
delante.

Qué cambió:

1. **Un solo borde por vecino, en su lado exterior.** El borde que mira al
   foco está *detrás* de la siguiente tarjeta, igual que el arte recortado
   (`Crop` ya lo modelaba: el vecino enseña su borde exterior). `CARD_GAP` a 0,
   de modo que entre dos tarjetas hay exactamente una barra. La banda pasa a
   ser continua: `│arte│arte│arte│`.
2. **La caída de escala es más marcada y desigual**: ancho `0.78 · 0.66^(d-1)`
   (antes `0.66 · 0.72`), alto `0.88 · 0.80^(d-1)` (antes `0.90 · 0.86`). El
   alto cae más deprisa a propósito: una tarjeta algo más estrecha pero casi
   tan alta lee como "copia más pequeña", no como "más lejos".
3. **La profundidad ahora es un número, no un interruptor.**
   `Treatment::Recessed(u8)` (1-100) por distancia `46 + 14·(d-1)`: el desenfoque
   (radio 1-3), el croma que se conserva (`40 - d/3` %) y la mezcla con el
   fondo (`d` %) crecen con ella. Antes todos los vecinos setrataban igual.
4. **El marco se apaga con la distancia**: los bordes a distancia ≥ 2 se pintan
   `Dim`, así que la profundidad se lee antes incluso de mirar el arte.
5. **`Cover::crop` tenía el brazo derecho del revés**: `Crop::right(k)`
   devolvía `w - k` columnas (con `w = 35, k = 19` sacaba 16), así que el
   vecino salía corto y con huecos en blanco dentro de la banda. Reescrito a
   "`keep_left` columnas por la izquierda y luego `keep_right` por la derecha",
   con test propio (`cropping_keeps_the_outer_edge_of_the_side_it_names`, que
   además falla si se revierte el arreglo). **No había ningún test de `crop`**,
   que es exactamente por lo que sobrevivió.
6. **La columna del plan depende de qué borde tenga la tarjeta**: el arte de una
   tarjeta derecha empieza en su propia `x` (su pieza empieza por el arte), y en
   `x + 1` si tiene borde izquierdo. Con `x + 2` para todas, los vecinos
   derechos se dibujaban una columna a la derecha de su celda.

### Verificación (Observed)

`cargo test --workspace`: **297 passed, 0 failed, 0 warnings**.
`wire_graphics_check`: 72/72 colocaciones sobre su tarjeta (el test comprueba
que cada tarjeta tiene el mismo esqueleto de columnas en **todas** sus filas, y
falla con `is not framed on row 5` al revertir el inset).
Rendimiento sin cambio (release): flecha 15-33 ms al primer byte, 43-53 ms
pintada.

### Lo que NO se hizo, y por qué

Solapar de verdad (meter la parte trasera de cada vecino **debajo** de la
tarjeta enfocada) no se puede hacer limpio en modo imagen: la parte oculta
tendría que ir en `z=-1` (bajo el texto) y la especificación de kitty dice que
con `z<0` un espacio repintado la borra; además con `z` negativo una celda con
fondo no transparente la tapa, así que en el caso mixto (foco sin portada, con
portadas los vecinos) el resultado depende del orden de pintado. El recorte
por columna (`Crop`) da el mismo resultado visual sin ese riesgo: la banda se
lee continua, la escala y el desvanecimiento marcan la profundidad y el foco
encima por su borde dorado, su tamaño y su tratamiento intacto.

## v17: siete en fila, la enfocada a resolución nativa y las seis en baja (Observed)

Pedido explícito: **al menos 7 tarjetas en fila** (1 enfocada en medio + 3 a cada
lado), la enfocada a **resolución normal** y las seis a **resolución baja con
blur, sin errores**. Tres cosas, dos de ellas bugs de verdad.

### 1. Siempre siete tarjetas

`carousel` saltaba la ranura cuando el índice no existía (`checked_sub`), así
que en el primer juego se veían 4 tarjetas y en el último 5. Ahora **clampea**:
`focus.saturating_sub(d)` y `(focus + d).min(len-1)`. En los extremos el juego
más cercano se repite detrás de sí mismo, que es lo que hace un carrusel al
llegar al final ("aquí se acaba la pila"), y como el desvanecimiento crece con
la distancia (`46 + 14·(d-1)`) la repetición se lee como profundidad y no como
un fallo. Con menos de 7 juegos en la biblioteca no se repite: se muestran los
que hay (`fan` corta cuando `2·d >= juegos`).

### 2. Laresolution es explícita, y por distancia

`transmit_max_side(depth)` = `260 - 2·depth` (límite 96 px): la enfocada sale a
**600x900** (los bytes del fichero, sin tocar) y las vecinas a **168, 140 y
112 px**. Además `fit_for_transmit` usa ahora `Triangle` en lugar de `Nearest`:
reduciendo a la tercera parte con el vecino se descartan dos de cada tres
píxeles, y el desenfoque convertía ese aliasing en bordes ondulados en vez de
suavizarlo.

**Bug 1 (el que rompía la "resolución normal")**: la capa memorizaba lo
transmitido por *fichero*, así que una portada vista primero como vecina se
reutilizaba al pasar a enfocada y llegaba a 112x168 en lugar de 600x900. La
clave ahora es `(fichero, tamaño transmitido)`: un envío por cada tamaño, que es
justo lo que hace falta. Comprobado en el cable
(`wire_graphics_check`): enfocada `[600x900]`, vecinas `[168, 140, 112]`.

**Bug 2 (artefactos)**: el recorte se calculaba con los píxeles del **fichero
original** mientras el payload ya iba reducido, así que el rectángulo caía fuera
de la imagen, kitty dibujaba la portada entera y la tarjeta salía **aplastada**
en vez de recortada. `Plan::wire_size()` dice el tamaño que realmente sale por el
cable y `source_rect()` trabaja sobre él. Test: el recorte tiene que caber en el
payload (`x + w <= payload_w`).

### 3. Píxeles fantasma en el pintor (el "sin errores" de verdad)

El diff por columnas (`tui-frame::row_patch`) memorizaba **el último** escape SGR
de cada celda. Una celda de portada es un medio bloque con **dos** escapes: `fg`
para el píxel de arriba y `bg` para el de abajo. Si sólo cambia el píxel de
arriba, el último escape (el fondo) es idéntico, la celda se tenía por igual y
**no se repintaba**: las portadas conservaban fantasmas del juego anterior.
`cells_of` ahora acumula el estado SGR completo (conecutando la tabla de estilos
por IDENTificador para no clonar cadenas por celda) y `slice_for` reemite ese
estado entero al empezar el trozo, no sólo el último escape — que además
perdía el color de fondo de cualquier medio bloque parcheado a mitad de fila.

Tests: `a_half_block_that_only_changes_its_top_pixel_is_repatched` (falla si se
vuelve al "último escape": el trozo tiene que ser corto, no la fila entera) y
`image_plans_tile_the_band_without_overlapping` exige ahora `plans.len() ==
layout.cols` en **todas** las posiciones, con 9 juegos en el fixture.

### Verificación (Observed)

`cargo test --workspace`: **299 passed, 0 failed, 0 warnings**.
`wire_graphics_check`: 129→72 colocaciones, todas sobre su tarjeta; resuelve
enfocada vs vecinas; sin `Clear(All)` que borre portadas.
Rendimiento (release, 190x46): arranque 129 ms; flecha 15-32 ms al primer byte y
76-106 ms pintada, con ~1 MB por tecla (la portada nativa de la enfocada).

## v18: "ahora se siente lento y con retraso al moverse de juego" (Observed)

La v17 arregló la resolución de la enfocada y, al hacerlo, rompió la
latencia: mandar la portada **nativa** en cada salto son ~950 KiB de base64 por
tecla (12 KiB el arranque, 0,8-1,7 MB por tecla), y con salida sincronizada el
frame no se presenta hasta que el terminal se ha tragado todo. Medido: 60-106 ms
por tecla con casi 1 MB, contra 45-53 ms y 110 KiB sin imágenes.

Solución: **`t=f`**. El protocolo permite mandar una *ruta* y que sea el terminal
quien lea y decodifique el fichero. Eso no lo da por hecho: **se verifica** al
arrancar, una vez.

* `probe_local_files()` escribe un PNG de 1x1 en nuestro propio directorio de
  estado y pregunta al terminal que lo cargue (`a=q,t=f,f=100,i=…`). Sólo la
  respuesta `OK` significa que puede leer ficheros; sin respuesta, con `ENOENT`
  o sin `KITTY_WINDOW_ID` se cae al camino de siempre (bytes por el pty). La
  lectura se hace con un hilo y un plazo de 300 ms, así que un terminal que no
  contesta no cuelga la app; lo único que cuesta es **una** pulsación al
  arrancar en ese caso (en kitty real la respuesta llega en milisegundos).
* Las portadas viajan como rutas (~200 bytes). Una portada que **ya es PNG** y es
  la enfocada se manda tal cual, sin tocarla. Todo lo demás (vecinas, que son
  reducidas y desenfocadas, y portadas en JPEG) se materializa **una vez** como
  PNG en `~/.local/state/rosadeck/covers/` y se reutiliza.
  *Bug que salió de aquí*: `png_path()` devolvía el fichero original también para
  las vecinas, así que iban **sin desenfoque** y a tamaño completo. Ahora el
  fichero es exactamente el payload del plan.
  *Y* el nombre de la copia se deriva de `(tratamiento, tamaño final)`, no del
  ancho de la tarjeta: el payload no depende de cuántas columnas tenga la
  tarjeta, y con el nombre viejo se escribía una copia nueva por distancia *y*
  por cada tamaño de terminal (36 ficheros, 3,1 MB, en vez de uno por
  profundidad).

Herramientas nuevas, que además evitan la carrera que hadía fallar la sonda al
azar (dos lectores sobre el mismo pty):

* `tools/pty_kitty.py`: pty que **responde** a la sonda `t=f` como kitty local
  (o con `ENOENT`, como uno que no puede), con un único lector que reenvía todo a
  una cola.
* `tools/pty_latency_check.py`: mide de tecla a frame pintado, y compara los dos
  modos.
* `tools/wire_graphics_check.py` ahora recorre **los dos** caminos y, con `t=f`,
  lee la cabecera PNG del *fichero* que el terminal va a cargar.

### Verificación (Observed)

| | arranque | tecla -> pintado | bytes por tecla |
|---|---|---|---|
| `t=f` (kitty local) | **19 KiB** | **15-27 ms** (mediana 20) | **1-2 KiB** |
| bytes por el pty (sin sonda) | 950 KiB | 59-93 ms | 671-1715 KiB |

`cargo test --workspace`: **299 passed, 0 failed, 0 warnings**.
`wire_graphics_check`: los dos modos, 72/68 colocaciones sobre su tarjeta,
enfocada 600 px y vecinas 112/140/168 px en ambos.
Coste de la primera vuelta con la caché vacía: se escriben 3 PNG por portada
(3,1 MB en total, una vez); a partir de ahí es un `stat` por neighbour.

## v19: "sigue lento, tarda 3 segundos" — medir en vez de suponer (Observed)

Reportado: 3 s por movimiento. Medido en release: **7-21 ms** (mediana 18) con
`t=f` y caché caliente. Factor ~40 sin explicar, así que antes de tocar código
se instrumentsa la sesión real, porque las dos hipótesis sacables desde fuera
("binary debug", "el terminal no puede leer ficheros") son las que explains el
retraso y ninguna se puede suponer.

### Lo que hace la app para poder responder

1. **La leyenda lo dice**: `portadas: rutas` (rápido), `portadas: bytes` (el
   terminal no lee ficheros) o `portadas: bloques` (sin protocolo gráfico), y
   `· BINARIO DEBUG (lento)` cuando el build no está optimizado. Es la diferencia
   entre "el código va lento" y "no estás probando el binario que crees".
2. **`~/.local/state/rosadeck/last-session.json`**, escrito antes del primer
   frame y cada 5 frames (también si la matan, no sólo al salir con `q`):
   binario, `debug_build`, si la sonda `t=f` funcionó, cuántos frames, cuántas
   portadas se enviaron y prepararon, bytes escritos, y los peores tiempos de
   `frame` y de la capa de imágenes.
3. Binario optimizado instalado en `~/.local/bin/rosadeck-library`. Ojo:
   **`~/.local/bin` no está en su `PATH`** (comprobado con `bash -lc`), así que
   hay que invocarlo por ruta o exportar el `PATH`; antes no había ningún
   `rosadeck-library` instalado en el sistema.

### Lo que sí estaba mal y se arregla

Codificar la portada de un vecino (decodificar 600x900 → reducir → difuminar →
PNG) son ~30 ms, y una tecla necesita dos a la vez: 60-130 ms en el camino de la
tecla. Ahora se hace **en los huecos**: cuando `poll` no devuelve nada y no hay
ninguna tecla pendiente (`poll(0)`), se construye el frame de la *siguiente*
posición y se materializan hasta 3 portadas (`Layer::prepare`). Nunca se empieza
con una tecla ya esperando: el trabajo se saca del camino de la tecla, no se
esconde dentro.

Medido: con caché en disco, la tecla baja de 52-82 ms a **7-21 ms** y a 1-2 KiB.

## v20: el lag desaparece al dar la vuelta completa (Observed)

Dato del usuario: **el retraso aparece al moverse y desaparece cuando ya se ha
pasado por todos los juegos y se vuelve al principio**. Eso localiza el coste con
precisión: no es el repintado (ocurre siempre), es **la primera vez que se ve
cada portada**. Todo lo que la app cachea por portada —el envío, la copia en
disco, la imagen en el terminal— sólo se paga una vez, y al final de la
biblioteca ya está todo pagado.

Con `t=f` ese trabajo son 1-2 KiB… así que si sigue costando, la sonda no está
funcionando y por eso se sigue mandando la portada nativa (~950 KiB) en cada
portada nueva. Se arregla **para el caso malo también**: que el trabajo salga del
camino de la tecla.

### Prefetch en los huecos (`Layer::prefetch`)

Cuando `poll` no devuelve nada **y no hay ninguna tecla pendiente**
(`poll(0)`), se construye el frame de la posición siguiente y se envían sus
portadas *sin colocarlas* (hasta 3). La tecla siguiente ya sólo coloca imágenes
que el terminal tiene. Las ids se asignan con la misma aritmética que `Layer::diff`
para que el frame siguiente las encuentre conocidas; test
`prefetching_leaves_the_next_frame_with_only_placements` (que falla si las ids
se desvían, si se reenvía algo, o si se coloca una imagen que el terminal no
recibió).

Medido en el camino lento (terminal que **no** puede leer ficheros, que es lo que
sospecho): antes 671-1715 KiB por tecla, ahora **1-1715 KiB** con mediana de 64 ms
— los picos de ~1,7 MB ocurren ahora en un hueco, no con la tecla pulsada.

## v21: la carátula se duplicaba tres veces en cada extremo (Observed)

Error reportado: al llegar al último juego su portada aparecía **tres veces
seguida** a la derecha, y la del primero tres veces a la izquierda. Era efecto
del clampeo de la v17: para mantener siete ranuras, el índice se saturaba y las
ranuras que faltaban se llenaban con el juego más cercano. Repetido, y con el
mismo arte a distinta escala y desenfoque, **parece un fallo de dibujo**, no una
estantería que se acaba.

Arreglo: una ranura sin juego es una **ranura vacía**. El sitio se conserva
(la banda no cambia de forma al llegar al final, que era el motivo de Saturar) y
dentro no hay nada: marco, sin artwork, con el borde aún más apagado que una
tarjeta lejana. Ningún juego se repite nunca.

Test `the_ends_of_the_library_are_empty_slots_not_repeated_covers`, leído **del
frame renderizado** (no recalculando la geometría aquí):
* ninguna portada aparece dos veces en la misma banda — se lee de los ficheros
  que el frame manda de verdad;
* la fila del medio de la banda sigue teniendo **8 bordes** (dos del foco, uno
  por vecino, vacíos o no), o sea siete tarjetas en todas las posiciones;
* sólo faltan ranuras en el lado que se acaba.

Comprobado con dientes: volviendo al clampeo el test falla con
`a cover is shown twice: [....png, ....png, ....png, ....png, Juego 4.png, ...]`.

`cargo test --workspace`: **301 passed, 0 failed, 0 warnings**.

## v22: en los extremos, sólo las portadas que hay (Observed)

Petición literal: quitar las tres "portadas sin usar" de cada extremo, que
Rosadeck no invente más carátulas de las que hay. Deshace tanto el clampeo de la
v17 (repetía la última tres veces) como las ranuras vacías de la v21 (eran un
hueco con marco). Ahora una ranura sin juego detrás **no se dibuja**, y
`band_offset` recentra lo que queda:

* primer juego → 1 enfocada + 3 a la derecha (4 tarjetas)
* en medio → 3 + 1 + 3 (7 tarjetas)
* último juego → 3 a la izquierda + 1 enfocada (4 tarjetas)

Coste honesto: al cruzar el borde (de la posición 8 a la 9 en una biblioteca de
12) la banda cambia de número de tarjetas y se recentra, así que las tarjetas
"entran y salen". Es lo que hacen las estanterías reales cuando se acaba la lista;
la alternativa (huecos) se veía peor.

Test `the_band_shows_only_the_games_that_exist`, leído del frame renderizado:
ninguna portada aparece dos veces, el número de bordes de la fila del medio
(= tarjetas + 1, porque el foco aporta dos) es el de las tarjetas que existen, y
hay exactamente una portada enviada por tarjeta.

`cargo test --workspace`: **301 passed, 0 failed, 0 warnings**.

## v23: sin marcos, y un nombre con iconos que sí existen (Observed)

Dos peticiones: quitar los marcos de cada portada, y mejorar el nombre de debajo
con iconos. La segunda escondía un bug que nadie había mirado.

### Los marcos

`Card::total_width()` pasa a ser el ancho del arte: sin bordes, sin esquinas, sin
columna de aire. Las tarjetas se tocan y el abanico se lee por escala, altura y
desvanecimiento, no por cajas. Consecuencias que hubo que arreglar en el mismo
paso:

* `CELL_ROWS_OVERHEAD` 5 → 4 (ya no hay fila de borde superior ni inferior), así
  que la portada enfocada gana una fila (31 en un terminal de 46).
* La fila del arte empieza en `y = 0` de la tarjeta y el nombre va justo debajo
  (`y = rows` y `rows + 1`), antes dos filas más abajo por los bordes.
* El plan de imagen: la columna del arte es `x + 1` para **todas** las tarjetas (ya
  no hay borde izquierdo que salte) y la fila del arte es `GRID_TOP + 1 + inset`
  (el `+1` del borde superior se va). Un error aquí desplaza la portada una fila
  o una columna respecto a su celda: es el bug que costó tres versiones.
* Los checks que buscaban el borde de cada tarjeta para validar la colocación
  dejan de tener sentido. Ahora se exige que **las celdas bajo la imagen estén en
  blanco** y que las tarjetas se sigan sin hueco (`b.col == a.col + a.cols`).

### Los iconos: `★` era una caja

Comprobarlo antes de usarlo lo cambia todo: `kitty.conf` **no fija `font_family`**,
así que kitty usa su fuente por defecto, **Noto Sans Mono**, y leyendo su `cmap`
(a pelo, sin `fonttools`, con `tools/fontcheck.py`):

* **no tiene `★` U+2605 ni `☆` U+2606** — la estrella que la interfaz ponía
  delante del nombre era una caja vacía desde el principio;
* no tiene ningún glifo Nerd Font (F005, F04B, F0000…);
* sí tiene `◉` `○` `▶` `▸` `◆` `·` `●`.

Nuevo módulo `icons.rs` con **dos juegos**:
* por defecto, `◉`/`○` (favorito) y `▶` (jugado: `▶ 3x`, `▶ nunca`), todo por
  debajo de U+26FF y fuera del uso privado;
* Nerd Font (Font Awesome: U+F005, U+F006, U+F04B) sólo con
  `ROSADECK_NERD_FONTS=1`, porque una Nerd Font es una elección, y activar la
  bandera sin ella produce tres cajas.

Test que exige que el juego por defecto no toque el rango privado. Guía para
poner una Nerd Font en kitty (con el nombre de una fuente que sí está instalada
aquí, comprobado): `docs/integrations/kitty-fonts.md`. Rosadeck sigue sin tocar
`kitty.conf`.

El nombre pasa a ser dos filas limpias: `◉ Mario Party 9` y
`Wii Europe  ▶ nunca`, con la región y las veces jugado.

### Verificación (Observed)

`cargo test --workspace`: **303 passed, 0 failed, 0 warnings**.
`wire_graphics_check` (los dos modos): 72/68 colocaciones, cada una sobre celdas
en blanco.
`tools/fontcheck.py`: la fuente real de kitty no tiene ni los glifos que la
interfaz usaba ni los Nerd Font; sí los nuevos por defecto.

## v24: "a este binario le falta lanzar en pantalla completa" (Observed)

El navegador **sí** sabe lanzar: Enter hace un preflight
(`rosadeck play --id <id> --dry-run`) y luego `rosadeck play --id <id> --yes
--quiet`, que es la ruta única documentada en F7. El fallo era de instalación:
`sibling_cli()` busca `rosadeck` **junto al navegador** y luego en el PATH, y en
esta máquina sólo estaba el navegador copiado a `~/.local/bin`, sin el CLI.
Resultado: `Err(NotFound)` y la línea de estado decía `play failed: No such file
or directory (os error 2)`; es decir, Enter no hacía nada y el mensaje no decía
por qué.

* Instalado `rosadeck` junto al navegador (`~/.local/bin/rosadeck`). Verificado
  en seco, sin ocupar la pantalla: `rosadeck play --id <id> --dry-run` →
  `rc=0` y `/usr/sbin/dolphin-emu -b -C Dolphin.Display.Fullscreen=True …`.
  El CLI lanza **en pantalla completa por defecto** (`profile.graphics.fullscreen
  .unwrap_or(true)`, `apps/rosadeck-cli/src/launch.rs:100`), así que no hace
  falta ningún flag desde el navegador.
* El fallo volver a ser imposible de entender: si el CLI no está, el estado dice
  `no encuentro el CLI rosadeck (busqué <ruta>); instálalo o ponlo en el PATH`, en
  vez de un error del sistema. Test que lo exige.

**Ojo**: `~/.local/bin` **no** está en el `PATH` de un login (comprobado), así que
`rosadeck` a secas no se encuentra: o se invoca por ruta, o se exporta
`export PATH="$HOME/.local/bin:$PATH"`. El navegador sí lo encuentra, porque
mira a su propio directorio primero.

## v26: la ventana de directorios, y lo que `[2J` borra las portadas (Observed)

«para configurar los directorios de las roms debe saltar una ventana no en la
esquina superior derecha», y después dos avisos: «cuando cierras el selector
desaparecen las portadas» y «desaparecen **las primeras 3**».

### La ventana

* `roots_dialog()` (`frame.rs`) devuelve `(fila, líneas)` y `build()` las
  **sustituye** sobre el frame ya compuesto: centrada en horizontal (márgenes
  izquierdo y derecho iguales) y en vertical, opaca y cuadrada (todas las filas
  miden `layout.width`, siempre). Ya no queda prompt en la esquina: la fila de
  filtros vuelve a su `/ find`.
* Contenido: `directorios de ROM · N añadidas`, una fila por raíz con su recuento
  real (`4 juegos`, `0 juegos`) o `no existe` en rojo, el prompt `ruta ▸ …` y la
  ayuda de teclas. Glifos verificados contra la fuente real (`╭ ╮ ╰ ╯ │ ─`
  existen en Noto Sans Mono; `⏎` **no**, por eso la ventana dice «Enter»).
* Una ventana modal **toma el teclado**: flechas, `Inicio`, `Fin` y `f` ya no
  mueven el estante de detrás.
* La lista son las raíces **configuradas**, no las que existen: una raíz cuyo
  disco no está montado tiene que poder quitarse desde la ventana.
* Si la terminal es muy pequeña la ventana **no sale** en vez de salir mal.

### El bug de verdad: `[2J` borra las imágenes de kitty

Primero se probó `Layer::clear()` al abrir (manda `a=d,d=A`, que **sí** libera los
datos): al cerrar se re-colocaba con `a=p` sin payload y las portadas no
volvían nunca. Se cambió a no hacer nada a mano y *parecía* arreglado… porque
**las dos herramientas de comprobación mentían**:

* `tools/pty_kitty.py` es un pty **falso**: sólo contesta el sondeo de `t=f`.
* `tools/graphics_store_check.py` (nuevo, modelo del protocolo) modelaba
  `a=d,d=A`, `a=d,d=N`, `a=t`, pero **no modelaba el `[2J]`**, y por eso daba
  «ok» con el bug presente.

La verdad la dio el spec, y después kitty real:

* Spec: «The clear screen escape code (usually `<ESC>[2J]`) should also clear all
  images» — o sea, también los **datos**.
* `tools/kitty_id_probe_child.py` corre **dentro de un kitty de verdad** y le
  pregunta (`a=p` sin `q=2`, que es como el spec dice que responde):
  ```
  a=p recién transmitido     -> OK
  a=p tras a=d,d=i (esconder) -> OK       (d=i conserva los datos)
  a=p tras a=d,d=A            -> ENOENT    (d=A los libera)
  ```
* `tools/kitty_app_cycle_child.py` repite la secuencia exacta de la app,
  `[2J` incluido, y ahí sí:
  ```
  id=1 tras cerrar -> ENOENT
  ```
  Es decir: el pintor repinta con `[2J`, kitty tira las imágenes, y el `a=p`
  posterior no encuentra nada. **Por eso el bug afectaba a todo repintado
  completo**: la ventana de directorios, un redimensionado, un cambio de tema, un
  `r`… no sólo a la ventana.

El arreglo es una regla, no un parche: `repaint_all(screen, layer)`
(`main.rs`) = `screen.invalidate()` + `layer.data_lost()`, y los siete sitios
que repintan pasan por ahí. `data_lost()` no borra nada (kitty ya lo borró):
sólo hace que la app deje de creer que tiene las imágenes, así que el siguiente
frame las **reenvía** (con `t=f` son 4-13 KiB de rutas, no megas de base64).

### Cómo se comprueba, que es lo que faltaba

Tres capas, y sólo la última se parece a lo que el usuario ve:

1. **Modelo del protocolo** (`tools/graphics_store_check.py`, con
   `--self-test`): lleva el flujo a un modelo de kitty y exige que toda
   colocación apunte a datos que existen. Ahora `[2J` limpia datos y
   colocaciones, y además exige que tras un repintado **se reenvíe** algo.
2. **Pregunta a kitty** (`tools/kitty_id_probe_child.py`,
   `tools/kitty_app_cycle_child.py`): lee las respuestas `OK`/`ENOENT` de un
   kitty real.
3. **Píxeles** (`tools/kitty_pixel_check.py`): kitty de verdad con control
   remoto (`-o listen_on=…` **más** `allow_remote_control=yes`; el flag
   `--listen-on` se ignora en 0.48 y el socket real lleva el pid), teclas con
   `kitten @ send-key` y capturas con `grim`. Mide tinta, color y un perfil de la
   banda de izquierda a derecha.

Con el arreglo:

```
al abrir  la ventana:  58.1 % de la banda cambia
al cerrar la ventana:   0.0 % de la banda cambia (media 0.0/255)
tinta: estante 64.5 · ventana 13.8 · vuelta 64.5
perfil estante: ......+##################+#########.....
perfil vuelta : ......+##################+#########.....
tras crecer la ventana: 0.0 % de cambio · tinta 64.5
RESULTADO: ok
```

Y **con dientes**: quitando el `layer.data_lost()` (el bug) la misma herramienta
dice

```
perfil vuelta : ......++++++#...........................
FALLO: al cerrar la ventana la banda pierde artwork (64.5 -> 5.9)
FALLO: la banda al cerrar no es la del principio
RESULTADO: FALLO
```

Trampa de medición: `pty_latency_check.py` mide «hasta 400 ms de silencio», así
que después de un repintado completo reporta ~810 ms aunque el frame con las
cuatro portadas salió a los ~150 ms y las 5 transmisiones siguientes son
prefetch en segundo plano. El primer byte tras la tecla es lo que importa.

Tests nuevos: `a_full_repaint_makes_the_covers_be_sent_again` y
`hiding_the_band_and_leaving_the_screen_are_different_commands` (capa),
`the_roots_editor_is_a_centred_window`, `the_window_lists_every_root_with_its_count`,
`a_tiny_terminal_gets_no_window_but_no_damage` y `the_window_swallows_navigation`
(frame/modelo). Workspace: **314 pasan, 0 fallos, 0 avisos**.

## v31: hyprgame → rosadeck (Observed)

«cambia el nombre del proyecto de hyprgame a rosadeck», con la carpeta y los
datos incluidos (decisión del usuario, no sólo los identificadores).

* **Texto**: 812 apariciones en 95 ficheros, cuatro formas y un solo criterio
  literal (`HYPRGAME` → `ROSADECK`, `HyprGame` → `Rosadeck`, `hyprgame` →
  `rosadeck`). Cubre paquetes de cargo, `use rosadeck_…::`, variables de
  entorno (`ROSADECK_ROMS`, `ROSADECK_COVERS`, `ROSADECK_NERD_FONTS`,
  `ROSADECK_NO_IMAGES`, `ROSADECK_THEME`, …), prefijos de los mensajes del CLI
  (`rosadeck: PREPARE_FAILED: …`), la marca de la pantalla (`ROSADECK ·
  RETRO LIBRARY`), las 812 líneas de documentación y los 15 scripts de `tools/`.
  **Lo que no se toca**: `backend-hyprland`, `hyprctl`, `Hyprland` — el backend
  habla con Hyprland y eso no cambia de nombre.
* **Carpetas y ficheros**: `apps/hyprgame-{cli,daemon,library,overlay}` →
  `apps/rosadeck-*`, `docs/integrations/hyprgame-keybind.md` →
  `rosadeck-keybind.md`, y el proyecto entero `~/hyprgame` → `~/rosadeck`.
* **Binarios**: `~/.local/bin/{hyprgame,hyprgame-library}` →
  `{rosadeck,rosadeck-library}` (los viejos borrados). El navegador busca a su
  hermano por nombre (`sibling_cli()`), así que el par tiene que ir juntos.
  `rosadeck --version` → `rosadeck 0.1.0`.
* **Datos del usuario, migrados con copia de seguridad**:
  `~/.config/hyprgame` → `~/.config/rosadeck` (perfiles de emulador, launchers,
  shaders) y `~/.local/state/hyprgame` → `~/.local/state/rosadeck` (biblioteca
  con favoritos y tiempo jugado, carátulas generadas, log del emulador,
  snapshots). Respaldo intacto en `~/.config/hyprgame-antes-de-rosadeck-<sello>`
  y `~/.local/state/hyprgame-antes-de-rosadeck-<sello>`, más un
  `tar` del proyecto antes de tocar nada.
* **Y un plan B en el código**: `dir_with_legacy(nuevo, viejo)` devuelve el
  nombre nuevo si existe, y si no el viejo. Los datos nunca se buscan «a lo
  bruto»: si el usuario lanza un binario viejo, o se le olvida una ruta, el
  nombre anterior sigue respondiendo en los seis sitios que dependen de disco
  (estado del navegador, `library.json` del CLI, perfiles del daemon, base de
  configuración del CLI, `roots_config_path`, instantáneas del backend).
  Test propio con los tres casos: sólo nuevo, sólo viejo, los dos.

Verificado después del renombrado, compilando y ejecutando desde
`~/rosadeck`: 317 tests pasan (uno nuevo), 0 avisos, `wire_graphics_check` 68/68,
`graphics_store_check` `ok`, latencia 8-25 ms (mediana 20) con `t=f`,
`kitty_pixel_check` `ok` (la ventana de directorios sigue devolviendo la banda
pixel a pixel) y `rosadeck play --dry-run` → `Result: VALID`. La biblioteca
sigue con sus 7 entradas de tiempo jugado: nadie empieza de cero.

Lo único que queda por fuera y **no** se puede renombrar desde aquí: si tu
`hyprland.conf` tuviera un atajo con `hyprgame-library`, habría que cambiarlo a
`rosadeck-library`. Comprobado: tu `hyprland.conf` no lo menciona.

## v30: por qué el proyecto pesaba 6 GB (Observed)

Pregunta: «¿por qué pesa 6 gigas?». Medido, no supuesto:

```
antes   6,0 GB  total
        5,5 GB  target/debug
               3,0 GB  target/debug/deps      ← 7+ copias del binario de test
               2,5 GB  target/debug/incremental ← 147 sesiones
          523 MB  target/release
        1,8 MB  el proyecto (apps + crates + docs + tools + f0)
```

**El proyecto no pesaba nada: 1,8 MB.** Los 6 GB eran la caché de compilación
de cargo, y dentro de ella dos cosas:

1. **Debuginfo completo en debug.** Un solo binario de test de
   `rosadeck-library` medía **137 MB**: el crate `image` (códecs de PNG, JPEG,
   …) entra entero y con tabla de símbolos completa. `cargo test` deja una copia
   por cada cambio, y había siete.
2. **`incremental/` con 147 sesiones** de compilación acumulada, 2,5 GB.

Arreglo, en `Cargo.toml` y con cifras medidas:

| | antes | después |
|---|---|---|
| `[profile.dev] debug` | `full` (por defecto) | `line-tables-only` |
| binario de test mayor | 137 MB | **58 MB** |
| `target/debug` tras compilar limpio | 5,5 GB | **946 MB** |
| `[profile.release] strip` | — | `debuginfo` |

`line-tables-only` deja los backtraces utilizables (archivo y número de línea) y
corta casi todo el debuginfo; `strip = "debuginfo"` quita lo que sobra de los
binarios instalados. El resto de ajustes son los de por defecto: ni `lto`, ni
`opt-level`, ni `incremental = false`, porque compilar es la mitad del trabajo de
este proyecto y Merece la pena pagar los 3 min 25 s de una compilación limpia.

**Los 446 MB de `target/release` son inevitables** si se quiere el binario: son
rlibs intermedios. Comprobado que el binario de 10,8 MB es *código*, no símbolos:
pasarlo por `strip` a mano sólo lo deja en 8,7 MB. Para subir el proyecto:

```
cargo clean          # fuera target/ — el proyecto queda en 1,8 MB
```

Y para reconstruir: `cargo build --release` (3 min 25 s desde cero) +
`install -m755 target/release/rosadeck{,-library} ~/.local/bin/`.

Nota: `~/roms` son 6,4 GB **fuera** del proyecto (tus ROMs y carátulas, nada que
subir), y `~/.local/state/rosadeck` pesa 4,8 MB (biblioteca, covers generados y
diagnóstico).

## v29: `▶ 4x` era un contador; ahora es tiempo jugado (Observed)

«cambialo mejor por el tiempo jugado y quita x y pon m (minutos) s (segundos) h
(horas)». `4x` contaba lanzamientos y no decía nada de cuánto se juega, que es lo
que uno recuerda de una tarde.

```
 Wii Europe  ▶ 4x          antes
 Wii Europe  ▶ 45s         ahora
 Wii Europe  ▶ 12m
 Wii Europe  ▶ 3h 20m
 Wii Europe  ▶ nunca       sin partidas
```

Decisiones:

* **El tiempo se mide donde el emulador está en pantalla**, que es el CLI: es
  el único que hace `spawn` + `wait`. `cmd_launch_timed()` devuelve
  `(código, segundos)` y `cmd_launch()` sigue siendo `cmd_launch_timed().0`, así
  que nadie más cambia. El navegador *no* mide: si lo hiciera, cronometraría su
  propia espera, que es lo mismo que hacer el trabajo dos veces.
* **Bug de conteo que sale por el camino**: se contaba una partida sólo con
  código 0, pero cerrar un juego normalmente es el código 19 («el juego corrió y
  terminó con un error»), o sea el final de cada partida. Casi ninguna sesión
  llegaba a la estantería. Ahora se cuenta `0` **o** `EXIT_EMULATOR_NONZERO`, que
  es exactamente «el emulador llegó a estar en pantalla».
* **Y el navegador duplicaba el registro**: él también llamaba a
  `record_play` después de esperar al CLI, así que cada partida sumaba dos. Ya
  no: recarga `library.json` y lo que manda es el CLI.
* `PlayStat` gana `played_secs` (`#[serde(default)]`, así que los ficheros
  antiguos cargan tal cual) y conserva `plays`, que no cuesta nada y dice cuántas
  sesiones produjeron ese tiempo.
* `played_time()`: segundos por debajo del minuto, minutos por debajo de la hora,
  horas y minutos por encima (`3h 20m`), y `nunca` cuando no hay nada. Por encima
  de 100 h se queda en horas. Un decimal de más era ruido: `229.0 MiB` ya se
  quedó en `229 MiB` en v27.

Tests: `play_time_is_shown_in_the_unit_that_reads` (12 casos, incluidos los
bordes 59 s / 60 s / 3599 s / 100 h) y `the_focused_card_shows_played_time` (la
tarjeta enfocada muestra `2h 6m`, `nunca` si no hay partidas y el `x` del
contador no aparece en la etiqueta), más el round-trip de `played_secs` en
`game-library`. Workspace: **316 pasan, 0 fallos, 0 avisos**.

Verificado de punta a punta sembrando `library.json` y arrancando el binario
instalado: `▶ 45s`, `▶ 1m` (95 s) y `▶ 2h 6m` (7.560 s) en la etiqueta. Las
estadísticas del usuario quedaron restauradas a su fichero original.

## v28: la cabecera, una línea (Observed)

«mejora esta parte», con la cabecera y los filtros pegados en el mensaje:

```
╔══════════════════════════════════════════════════════════╗
║ ROSADECK RETRO LIBRARY              12 shown · 12 roms · 0 fav ║
╚══════════════════════════════════════════════════════════╝
   ALL 12   SNES 2   N64 1   WII 5   3DS 4            / find
──────────────────────────────────────────────────────────────
```

Ahora:

```
 ROSADECK · RETRO LIBRARY              12 shown · 12 roms · 0 fav
   ALL 12   SNES 2   N64 1   WII 5   3DS 4            / find
──────────────────────────────────────────────────────────────
```

* La caja doble (`╔═╗` / barra / `╚═╝`) ocupaba **tres filas** para decir once
  palabras y era el marco más pesado de una pantalla que ya no tiene ni un marco
  alrededor de las portadas (v23). Al contraryo, el marco competía con la única
  cosa que merece mirarse. Ahora la marca va en la línea de arriba, en acento y
  oro, con los recuentos a la derecha, y la regla fina bajo los filtros es lo que
  separa el chrome del arte.
* En una terminal diminuta los recuentos son lo prescindible: si no caben con un
  espacio de separación, se quedan sólo el nombre y la línea se corta sin pegar
  números a letras (bug encontrado por el test de ancho exacto).
* `GRID_TOP` 5 → 3 y `CHROME_ROWS` 9 → 7: las dos filas liberadas van a la
  portada, que en la captura se ve claramente más grande. El navegador queda con
  **tres** filas de chrome arriba y **dos** abajo.
* Fuera `glyph::D` y `glyph::DV` (`═`, `║`): ya no se usan, y el test de
  «todos los glifos miden una columna» se queda con los que existen.

Tests: el ancho exacto a 20x12 (que cazó el header pegado a los números),
`frame_has_header_grid_detail_and_legend` (la primera fila es la marca y no hay
`╔`), `layout::users_terminal_gets_a_big_focus_and_neighbours` (`cover_h` 33 →
35) y el resto se adapta con `GRID_TOP`. Workspace: 314 pasan, 0 fallos, 0
avisos; `wire_graphics_check` 68/68; latencia 6-23 ms (mediana 17) con `t=f`;
`kitty_pixel_check` `ok`.

## v27: el pie, minimalista (Observed)

«este texto hazlo mas minimalista», con el pie entero pegado en el mensaje:

```
  LUIGI'S MANSION Wii Europe
  size 229.0 MiB · plays 4 · emulator dolphin ✓ · cover Luigi's Mansion.png image
  file /home/rosa/roms/wii/Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz
  ↑↓←→ move  ⏎ play  f fav  / find  1-5 plat  0 all  d roms  r rescan  w theme  q quit  portadas: bytes
```

Ahora:

```
  229 MiB · dolphin ✓
  ←→ move  ⏎ play  f fav  / find  d roms  q quit
```

Decisiones, y por qué:

* **Título, plataforma, región y jugadas no se repiten**: ya están bajo la
  portada (`○ Luigi's Mansion` / `Wii Europe ▶ 4x`). Era lo más ruidoso de la
  línea y no aportaba nada.
* **El nombre del fichero de la portada y la ruta del ROM salen del pie.** Una
  portada que decodifica es el caso normal y no necesita nombre; la que **no**
  decodifica sí se nombra (`cover X.png unreadable`, en rojo), porque entonces el
  carrusel está dibujando un marcador generado y hay que decirlo. La ruta del
  ROM aparece en la línea de estado **cuando un lanzamiento falla**, que es
  cuando hace falta.
* **Etiquetas `size ·` y `emulator ·` fuera**: con un número y un nombre ya se
  entiende; sólo tenían sentido cuando la línea tenía cuatro campos. Igual con
  los decimales: `229 MiB`, no `229.0 MiB` (se mantienen por debajo de 10).
* **La leyenda se queda con seis teclas**: `←→ ⏎ f / d q`. Los filtros por
  plataforma (`1`-`5`, `0`), `r` (rescan) y `w` (tema) siguen funcionando y
  quedan en la documentación, que es donde deben estar: una fila con diez pares
  de teclas era un muro, no una leyenda. `⏎` sigue diciendo `needs emulator`
  cuando ese juego no se puede lanzar.
* **`portadas: rutas|bytes|bloques` se muda a la línea de estado, a la derecha**:
  es un *estado*, no una tecla, y la izquierda queda para el último mensaje
  (`played X`, `added …/quitado …`, errores). Sigue visible en reposo, que es
  cuando hace falta para juzgar un «va lento».

Efecto colateral que se pidió sin pedirlo: el pie pasa de seis filas a cuatro
(`CHROME_ROWS` 11 → 9), y esas dos filas van a la portada, que es lo que la
vista mira. En la captura de `tools/kitty_pixel_check.py` la carátula enfocada
crece visiblemente y todo lo demás sigue igual (`RESULTADO: ok`: al cerrar la
ventana de directorios la banda vuelve **pixel a pixel**).

Tests tocados por el recorte, todos actualizados con su motivo: el pie son
cuatro filas (`status_line_is_actually_painted`), el título se lee bajo la
portada y el pie no imprime la ruta (`frame_has_header_grid_detail_and_legend`),
el cursor se sigue en la etiqueta de la tarjeta (`selected_cell_is_highlighted`),
una portada que decodifica no ocupa sitio
(`a_cover_that_cannot_be_decoded_is_named_as_unreadable`,
`image_mode_skips_generated_covers`), la portada enfocada gana 2 filas
(`layout::users_terminal_gets_a_big_focus_and_neighbours`), y `human_size`
redondea sin decimales de más (`format_helpers`). Workspace: 314 pasan, 0
fallos, 0 avisos.

## v25: `d` configura los directorios de ROM, dentro del navegador (Observed)

Petición: «con la tecla `d` puedas configurar los directorios de las roms», sin
tocar ficheros de configuración a mano y sin salir del navegador.

Decisiones:

* El editor vive en el **modelo puro** (`browser.rs`), igual que la búsqueda:
  `roots_input`/`roots_text` y `Key::Dirs` → `BrowserKey::SetRoot(String)`. El
  runtime sólo valida, guarda y reescanéa (`set_root`, `main.rs`). Ni una línea
  de hyprctl/sockets/sysfs, ni estado de terminal en el modelo.
* Persistencia en **`$XDG_CONFIG_HOME/rosadeck/roms`** (o `~/.config/rosadeck/
  roms`): una ruta por línea, `#` comentarios, `~` expandido. Es el único
  fichero que Rosadeck escribe fuera de su directorio de estado, y sólo cuando
  el usuario lo pide con una tecla. Se puede editar a mano después.
* **Repetir una ruta la quita.** El mismo gesto añade y quita, y la línea de
  estado lo dice (`añadido …` / `quitado …`). Una ruta que no existe se acepta
  y se avisa (`la carpeta no existe todavía`): un disco sin montar es una
  entrada legítima, y `default_roots` la salta mientras no esté (`is_dir()`),
  sin romper el escaneo.
* Orden de raíces: `~/roms`, luego las configuradas en el orden en que se
  añadieron, luego `$ROSADECK_ROMS`, y `--roms` delante de todo para esa
  ejecución. Deduplicadas. Es también el orden de escaneo.
* El CLI lee el mismo fichero (`rosadeck library` usa `default_roots`), así que
  navegador y CLI no pueden discrepar.

**Bug encontrado midiendo, no leyendo**: `/` seguía siendo el atajo de búsqueda
dentro del editor, porque el brazo `(KeyCode::Char('/'), _) => Key::Search` no
miraba el flag de escritura y estaba *antes* del brazo genérico
`(Char(c), true) => Key::Type(c)`. Escribir `/tmp/opencode/d-roms` encendía la
caja de búsqueda y se comía el resto. Lo mismo pasaba con los dígitos
(`1`–`5`, `0`) y con `q`, que el runtime intercepta antes de `map_event`: una
ruta con un `1` saltaba de plataforma y una con `q` cerraba el navegador. Ahora
el flag de escritura se calcula una vez (`typing = searching || roots_input`) y
**todo** carácter es texto mientras haya un editor abierto (`main.rs:186`).

Medido en pty (`tools/pty_kitty.py`, 150x40, build release):

```
d                     -> prompt del editor (en v26, una ventana centrada)
/tmp/opencode/d-roms  -> se escribe entero (antes: «/ tmp» y búsqueda)
/tmp/opencode/d-roms⏎ -> 12 → 13 juegos, WII 5 → 6, detalle «D测试 Wii»
repetir la ruta ⏎     -> 13 → 12, el fichero queda sólo con la cabecera
/mnt/no-existe ⏎      -> «añadido /mnt/no-existe · 13 juegos · la carpeta no existe todavía»
Esc tras escribir     -> no escribe nada
arranque siguiente     -> 13 juegos (persiste)
```

Tests: `typing_a_path_never_trips_a_shortcut` (incluye `/`, `d`, `q`, `r`, `w`,
`f`, `1`, `0`, `~`, espacio y `ñ`; comprobado **con dientes**: revirtiendo el
orden de los brazos vuelve a fallar diciendo `left: Some(Search)`),
`the_roots_editor_types_paths_and_never_eats_shortcuts` (cada tecla es texto,
`Enter` vacío no inventa raíz y no cierra el editor, `Esc` cancela, buscar y
editar raíces no se pisan) y `configured_roots_round_trip_through_the_file` +
`only_a_leading_tilde_is_expanded` (ida y vuelta del fichero; espacios del
medio son parte de la ruta, sólo se recortan los bordes; `~user` **no** es
`$HOME`). Workspace en esta v25: 309 pasan, 0 fallos, 0 avisos. Ese test del
prompt en la esquina se retired en v26, que es justo lo que el usuario pidió
cambiar.

Pendiente de decidir con el usuario: la petición original era «los directorios
de los emuladores» (`~/.config/rosadeck/emulators`); esta iteración cubre las
ROMs, que fue la corrección. El mismo editor podría cubrir los emuladores con
otra tecla si se quiere.

## v7: "al pasar por New Super Mario Bros Wii se buggea" (Observed)

El bug era **solo** de la capa de imágenes, y por eso aparecía exactamente en el
único juego con portada. Contrastando el código con la especificación del
protocolo gráfico de kitty (no suponiendo) salieron tres fallos:

1. **Faltaba `q=2` (quiet) en `a=p`.** La especificación dice que al indicar
   un `i=` el terminal **responde por stdin** (`ESC_Gi=1;OKESC \`). Esos bytes
   llegaban al lector de teclado de crossterm como pulsaciones espurias: por eso
   la UI se rompía justo al pasar por ese juego y "no dejaba volver". Ahora
   **todos** los comandos llevan `q=2` (verificado en vivo: 201 comandos, 0 sin
   `q=2`), y hay un test que lo exige para cada comando emitido.
2. **Faltaba `C=1`** (no mover el cursor). Tras colocar una imagen el terminal
   mueve el cursor `c` columnas y `r` filas; si eso se sale de la pantalla la
   especificación dice que la posición queda *indefinida* (y en kitty la pantalla
   hace scroll). Con `C=1` el cursor no se mueve nunca.
3. **Faltaban los ids de colocación (`p=`).** Ahora mover una portada es un
   `a=p` con los mismos `i,p`: kitty *reemplaza* la colocación en el sitio, sin
   borrarla y sin parpadeo. Al salir de la banda se oculta con `d=i` (minúscula:
   quita la colocación pero **conserva los datos**), así que volver a verla son
   ~30 bytes en vez de 800 KB.

Hueco extra cerrado: `forget()` olvidaba los ids **sin borrar** las colocaciones
— una portada que desapareciera tras un `r` habría quedado pegada en pantalla.
Ahora `forget()` escribe los borrados (`forget(&mut out)`), e `invalidate()`
marca la capa como desincronizada para que el siguiente frame la reconstruya
entera (redimensionar ya no puede dejar imágenes huérfanas).

Verificado en vivo (pty 190x46, kitty, con tu portada): ida y vuelta idéntica
para 1, 5, 9 y 10 flechas; con redimensionado 190→150→190: 0 comandos sin
`q=2`, 2 ocultados, 3 recolocaciones, 195 chunks de payload solo la primera vez;
el detalle sigue diciendo `cover New Super Mario Bros. Wii.png image`.

## v6: "al llegar a SNES se buggea y no vuelve atrás" (Observed)

Síntoma exacto del usuario: al llegar a los juegos de SNES la UI se rompía, no
se podía volver atrás y la portada de *New Super Mario Bros. Wii* no aparecía.

**Herramienta nueva: modelo de terminal en Rust** (`tui-frame::vt`). Mis
validaciones anteriores (comparar bytes) no servían: el mismo output erroneous
puede parecer plausible. El modelo interpreta lo que la terminal interpretaría
(cursores, SGR, ED/EL, CRLF, UTF-8 de a un carácter por celda) y permite
comparar **caracteres y estilos** de la pantalla real. Con eso:

**Causa 1 — el parche no reponía el estilo heredado.** `row_patch` escribía un
reset al principio del fragmento, pero no re-emitía el escape que *establecía* el
color en la columna parcheada: si el cambio empezaba en la columna 70 y su color
venía de un escape en la 69, esa columna salía con el color por defecto. Ahora
`slice_for()` arranca en el último escape en o antes de la columna inicial.

**Causa 2 — el parche no cubría toda la secuencia coloreada.** Al mover la
selección cambian escapes pero **no** los glifos: el parche cubría el primer
carácter tras el escape y dejaba el resto de la portada con el color viejo
(borde dorado sigue puesto → "no vuelve atrás" y el dibujo se ve corrupto). Un
escape SGR.style aplica a *todos* los glifos hasta el siguiente escape, así que
el parche ahora se extiende hasta justo antes del siguiente.

**Causa 3 — `prev` guardaba la línea sin truncar** mientras la terminal recibía
la truncada: una columna final se quedaba obsoleta para siempre. `Screen`
guarda ahora exactamente lo que escribe.

Herramientas de verificación (las dos valen oro):
- `crates/tui-frame::vt` — modelo de terminal, con `first_difference()`.
- Test `painting_real_frames_converges_to_the_last_one`: frames **reales** del
  carrusel (selección yendo y viniendo) pintados por `Screen` y comparados con un
  repintado completo del último frame, en 3 tamaños de terminal.
- `tools/vtterm.py` (extraído de `vt_compare.py`) — lo mismo sobre una sesión
  real de pty.

Verificado en vivo (pty 190x46, kitty, con la portada real): **ida y vuelta
idéntica al inicio** para 1, 2, 4, 5, 7, 8, 9 y 10 flechas (antes divergía con
96 celdas con estilo dorado residual). El juego 5 envía su portada (195 chunks) y
el detalle dice `cover New Super Mario Bros. Wii.png image`.

Nota: el fallo del "no aparece la portada" era la **consecuencia** de la 2 — al
volver, la celda quedaba con el estilo del frame anterior y la imagen no se
recolocaba donde debía.

## v5: carrusel de una fila + portadas nativas (Observed)

**"Se ve a una resolución bajísima"** — diagnóstico correcto y con explicación:
el bloque medio `▀` es **un píxel por columna** de carácter, así que una portada
de 600x900 se veía reducida a 18x24 píxeles. Dos arreglos:

1. **Carrusel de una sola fila**: `layout.rs` da *todas* las filas libres a la
   portada (en 190x46: `cover_h = 30`, o sea 45x30 celdas = **45x60 píxeles**, un
   62 % más de resolución lineal) y `cols` es cuántas caben (3). Se navega con
   ←/→ y la banda se desplaza (`window_for`) solo cuando haría falta.
   Arriba/Abajo saltan una pantalla.
2. **Imagen nativa vía protocolo gráfico de kitty** (`images.rs`): la portada se
   **transmite como imagen** y la muestra kitty con filtrado suave a la
   resolución real del terminal. Ya no hay bloques en esa celda (queda en blanco
   para que la imagen se vea) y el resto de juegos sin portada siguen con
   bloques generados.

Detalles que importan:

- `a=t` (transmitir) separado de `a=p` (mostrar): **el archivo se envía una
  sola vez** y al desplazar el carrusel solo se re-coloca (~30 bytes). Medido en
  pty: primer sighting 807 KB (195 chunks), siguientes flechas 16-45 KB de texto y
  **0 payloads**.
- Colocación **virtual** (`z=-1`): ni `Clear(All)` ni el pintor por diferencias
  borran las imágenes.
- Ids estables por archivo; borrar es `a=d,d=I,i=<id>`; al salir (o al lanzar el
  emulador) un único `a=d,d=A`.
- Un archivo ilegible o >6 MB se salta sin romper nada, y no se recuerda.
- Detección: `TERM=*kitty*` o `KITTY_WINDOW_ID`; se desactiva con `--no-images`
  o `ROSADECK_NO_IMAGES=1` (verificado: 0 payloads).
- El panel de detalle dice el modo: `cover <archivo> image` o `... blocks`, o
  `cover generated`.
- Cabecera, chips, detalle y leyenda ocupan **todo el ancho** del terminal; la
  banda va centrada dentro (`band_indent`).

Bug encontrado de paso: en el carrusel la flecha derecha se quedaba clavada al
final de la ventana (la regla de "no cruzar de fila" era de la rejilla). Ahora el
modelo es lineal: ←/→ = ±1 juego, ↑/↓ = ±pantalla.

Tests nuevos: geometría del carrusel en 9 tamaños × 5 tamaños de biblioteca,
centrado de la banda, protocolo de transmisión (cabeceras, chunks, `a=p` sin
payload), modo imagen vs bloques, limpieza, y `tools/vt_compare.py` sigue dando
PASS (el parity por columnas sigue siendo exacto con imágenes activas).

## v4: portadas en cualquier formato, nombradas por juego (Observed)

**Formatos.** `image` 0.25 pasa a `features = ["avif-native"]`: se activan todos
los decodificadores que trae (png, jpeg, webp, avif/ravif, gif, tiff, bmp, tga,
ico, qoi, pnm, dds, exr, hdr, farbfeld) **más la decodificación AVIF nativa** vía
el dav1d 1.5.4 del sistema. Verificado con un test que *codifica* una imagen
magenta en cada formato y la decodifica de vuelta por `Cover::from_file`
(comprueba geometría y píxeles):

```
decode OK: png jpg webp gif tiff bmp tga ico qoi pnm ppm avif
sin encoder aquí: dds exr hdr   (el decodificador sí está; el test no los genera)
```

Además `ART_EXTENSIONS` (24 extensiones,incl. heic/heif) es la lista que usa el
buscador de portadas, y una prueba E2E comprueba que una portada **WebP con
pérdida** y otra **AVIF**, nombradas por el juego, llegan al frame con sus
píxeles (`38;2;0;255;0` + bloques medios).

**La imagen se llama como el juego, en `~/roms/covers/`.** Decisión del usuario:
la carpeta de portadas es `<rom-root>/covers/` (`/home/rosa/roms/covers/`) y se
busca **primero**, para que un drop-in explícito gane siempre. Orden completo:

1. `~/roms/covers/` ← principal
2. `~/roms/covers/<platform>/` (la misma carpeta ordenada)
3. junto al ROM
4. `~/roms/art/<platform>/` (legado, se respeta)

`ROSADECK_COVERS` (separado por `:`) sustituye la principal. Nombres
aceptados, en orden y sin distinguir mayúsculas: **nombre del juego**
(`Luigi's Mansion.png`) y nombre del ROM
(`Luigi's Mansion (Europe) (En,Fr,De,Es,It).png`). Cada carpeta se lista una
sola vez por llamada para el fallback sin mayúsculas.

`rosadeck art` dice qué crear exactamente:

```
  --  Luigi's Mansion                              Wii
        crea: /home/rosa/roms/covers/Luigi's Mansion.png
  ok  New Super Mario Bros. Wii   /home/rosa/roms/covers/New Super Mario Bros. Wii.png
```

**Rendimiento.** Resolver una portada son ~100 `stat` en 4 carpetas: la TUI lo
memoiza (`CoverCache::art_path`), así que el frame no paga I/O tras el primer
pintado; la caché se invalida al redimensionar o al pulsar `r`. El panel de
detalle indica el origen: `cover New Super Mario Bros. Wii.png` o
`cover generated`.

**Verificado en vivo** con la portada real del usuario (600x900 PNG):
`rosadeck art` la encuentra y el frame crece **+4,2 KB** de SGR de color frente
a la carátula generada (1.252 vs 1.135 escapes fg, 883 vs 778 bg) → se está
pintando de verdad, y `tools/vt_compare.py` sigue dando PASS con ella.

## v3: sin parpadeo + Pywal (Observed)

**Parpadeo (bug reportado: "cada flecha parpadea la UI")**. Causa medida: cada
tecla repintaba la pantalla entera (`Clear(All)` + 58 KB) y el repintado se
veía a medio camino. Tres arreglos, todos en `tui-frame` y verificados en pty:

1. `Screen`: diff por filas — solo se reescriben las filas que cambian; frame
   sin cambios = **0 bytes**. Nada de `Clear(All)` por tecla.
2. `row_patch`: diff por columnas dentro de la fila. Una fila de estantería son
   190 columnas de carátula (~1,6 KB); mover el cursor solo cambia 2 bordes, así
   que se reescriben esas columnas. Flecha Abajo: **55 KB → 8 KB**.
3. Salida sincronizada (DEC 2026) cuando el terminal la soporta (kitty, foot,
   WezTerm): el terminal presenta el frame entero de una vez, sin parpadeo.
   Desactivable con `ROSADECK_NO_SYNC=1`.

Medido en pty 190x46 (13 juegos, carátulas generadas): primer frame 59,8 KB;
flechas 8-20 KB; escribir en la búsqueda 0,4-1,3 KB; favorito 206 bytes; sin
teclas 0 bytes. Los cambios de filtro (`1-5`) sí repintan de verdad porque la
lista cambia: 48-60 KB, correcto.

Prueba de corrección: `tools/vt_compare.py` reproduce la sesión en pty, la
parchea fila a columna, la luego fuerza un repintado completo (rebote de
tamaño) y compara ambas pantallas con un emulador VT propio: **idénticas**
(PASS), o sea que el parcheo no deja celdas obsoletas ni fuga de estilos.
También lo verifica con filtro + búsqueda + flechas.

**Pywal**. `pywal.rs` lee `$XDG_CACHE_HOME/wal/colors.json` (layout moderno
anidado y legacy plano), y de ahí sale la paleta de los 6 roles. No confía a
ciegas: pywal mantiene `color1..6` oscuros a propósito, así que cada rol
busca la variante brillante y se valida contraste **WCAG** contra
`special.background` (texto ≥ 4.5, acentos ≥ 3, decorativos ≥ 1.5). Un
paleta gris plano cae al `special.foreground` para no quedar ilegible.
`theme.rs` resuelve estilos a través de la paleta (nada de SGR fijos), con
truecolor o cuantización xterm-256 según el terminal. En vivo: tu
`~/.cache/wal/colors.json` (wallpaper `tayama.jpg`) → acento `#57A19F`, chips
`#5ED8D8`, bordes mezclados.

Flags: `--theme auto|pywal|system` (o `ROSADECK_THEME`), `w` recarga la paleta
al vuelo, y como el bucle hace `poll` en vez de bloquearse, un `pywal -i` nuevo
se detecta solo (mtime de `colors.json`) sin tocar el teclado. En estado
plano (`--no-color`) se ignora.

Extras en `tui-frame`: `parse_hex` (formas pywal), `blend`, `luminance` y
`contrast` (WCAG), con tests.

## Bug de UI: raw mode sin CR (§ UI-bug)

Síntoma reportado: "texto solapado o cortado" en kitty 190x46, aunque el
`--dump` se veía perfecto. Dos causas reales, ambas verificadas en pty:

1. **LF sin CR (causa raíz)**. `crossterm::enable_raw_mode()` llama
   `cfmakeraw()`, que borra `OPOST`: en raw mode `\n` es solo LF. Medido en
   el stream de salida: `CRLF pairs: 0, bare LF: 16` y el corte literal
   `b"...query: \n\xe2\x94\x80..."`. Cada fila arrancaba en la columna donde
   terminó la anterior → escalera de texto. El `--dump` no lo suffería
   porque fuera de raw mode la tty traduce `\n`→`\r\n`.
2. **`Clear(All)` no Bring-to-home**. Sin `MoveTo(0,0)` el frame 2+ se
   escribía encima del frame 1.

Solución: nuevo crate puro `tui-frame` (`frame_bytes`/`write_frame`) con
dos reglas que ninguna TUI puede saltarse — **todo salto es CRLF** y
**ninguna línea llena la última columna** (deferred wrap=row saltada).
Defensa extra: `safe_width` trata un reporte de 0 columnas como inválido
(fallback 80), porque una pty sin dimensionar colapsaba el frame a 1 carácter.
Bonus del mismo diagnóstico: kitty reporta *key releases*, así que sin
filtrar `KeyEventKind::Release` cada tecla movía **dos** filas. Ambos efectos
visibles en pty ahora: 2 pulsaciones = 2 filas.

`tools/pty_frame_check.py` reproduce la comprobación (tamaño arbitrario,
teclas, bytes CRLF). Resultado tras el fix en 190x46 y 60x20: `bare_LF=0`,
selección en columna 0, `▶` alineado en el overlay.

Regresión cubierta por tests: 4 unitarios en `tui-frame` (CRLF, última
columna, terminales diminutos, forma real de frame) + audit source-level en
ambas TUIs que prohíbe `write!` directo a stdout.

## Decisiones descartadas

Scraping/red (offline-first); artwork obligatorio; biblioteca en SQLite
(JSON basta); `.zip` universal; SIGTERM elegante al hijo (F6 heredado);
descubrimiento de emuladores por escaneo (perfiles explícitos).

## Cómo ver carátulas reales

`rosadeck-library` busca, por orden: `<rom>.png|jpg|jpeg` junto al ROM, luego
`~/roms/art/<platform>/<nombre>.png|jpg`. Cualquier imagen vale (recorta al
centro y reescala). Sin carátula, la interfaz genera una para que la
estantería nunca tenga huecos.

## Riesgos / pendiente

Sin lanzamiento real ejecutado (proponer: Luigi's Mansion 30 s);
RetroArch sin validar; `.zip` en Dolphin por confirmar; sin HDMI para
combo display+emulador; sin carátulas en la colección real (el camino de
arte está probado con PNG sintético, falta probarlo con arte real del
usuario); `image` añade ~8 s de compilación la primera vez.
