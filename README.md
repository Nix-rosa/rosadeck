# Rosadeck

Un gestor de pantallas y emuladores para **Wayland/Hyprland**, con una
biblioteca de juegos en terminal que se lee como un carrusel de carátulas.

> un espacio para jugar emuladores de una manera rapida y facil, sin complicaciones

```
rosadeck          # abre el navegador de la biblioteca (lo normal)
rosadeck --help   # el CLI: play, library, displays, modes, duplicate, …
```

## Qué hay dentro

| pieza | qué hace |
|---|---|
| `rosadeck` | el CLI: lanza juegos, clona o extiende una pantalla, restaura el perfil guardado. Sin argumentos, **abre el navegador**. |
| `rosadeck-library` | el navegador: carrusel de carátulas con el protocolo gráfico de kitty, busca y filtra, y lanza en pantalla completa. |
| `crates/backend-hyprland` | la única parte que habla con `hyprctl`. Todo lo demás es puro y se testea sin sesión gráfica. |
| `crates/game-library` | escaneo de ROMs, títulos, carátulas por convención, favoritos y tiempo jugado. |

## Requisitos

- **Wayland + Hyprland** (el backend habla `hyprctl`; DisplayLink/DRM/EDID están
  soportados pero el camino probado es Hyprland).
- **kitty** como terminal: las carátulas viajan por su protocolo gráfico, y el
  navegador necesita `t=f` (leer ficheros del disco) para que la estantería
  responda a las flechas.
- **Rust estable** para compilar; `image`, `crossterm`, `sha2`, `serde`.

## Compilar e instalar

```bash
git clone <tu-repo> ~/rosadeck
cd ~/rosadeck
cargo build --release
install -m755 target/release/rosadeck target/release/rosadeck-library ~/.local/bin/
```

`~/.local/bin` **no** está en el `PATH` de una sesión de login en la mayoría de
las distribuciones: o invócalo por ruta, o añade

```bash
export PATH="$HOME/.local/bin:$PATH"
```

## Romanes de juego

`~/roms/<platform>/` por convención, con las carátulas en `~/roms/covers/`
nombradas como el juego (cualquier formato que sepa decodificar `image`). Se
ejecuta desde el navegador con `d` para añadir directorios, que se guardan en
`~/.config/rosadeck/roms`.

```
~/roms/
  wii/Luigi's Mansion (Europe) (En,Fr,De,Es,It).rvz
  snes/…
  covers/Luigi's Mansion.png
```

## Teclas del navegador

`←→` mover · `⏎` jugar a pantalla completa · `f` favorito · `/` buscar ·
`d` directorios de ROM · `q` salir. Los filtros por plataforma (`1`-`5`, `0`),
el reescaneo (`r`) y el tema (`w`) están en
[`docs/integrations/rosadeck-keybind.md`](docs/integrations/rosadeck-keybind.md).

## Documentación

- [`docs/research/`](docs/research/) — la bitácora: qué se rompió, cómo se
  midió y qué se decidió en cada fase (F0 a F7). Es donde está el razonamiento,
  incluidos los errores que costaron tiempo.
- [`docs/integrations/`](docs/integrations/) — keybinds, kitty, quickshell.
- [`tools/`](tools/) — las sondas que verifican de verdad: `wire_graphics_check`
  (las carátadas caen en su tarjeta), `kitty_pixel_check` (píxeles de un kitty de
  verdad), `pty_latency_check` (tecla → frame), `graphics_store_check` (el modelo
  del protocolo gráfico).

## Diseñado para no depender del terminal

Todo lo que se puede decidir sin una sesión gráfica se decide sin ella: los
crates de lógica son puros, la mutación de pantallas vive únicamente en
`backend-hyprland`, y las herramientas de `tools/` hablan con la aplicación a
través de un pty. `cargo test --workspace` corre sin pantalla y comprueba
posiciones, anchos de columna, protocolo gráfico y las rutas de configuración.

## Licencia

MIT — ver [`LICENSE`](LICENSE).
