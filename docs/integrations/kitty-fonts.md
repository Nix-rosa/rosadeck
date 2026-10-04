# Fuentes e iconos de Rosadeck

Guía, no configuración aplicada: Rosadeck **no** toca tus ficheros de configuración.
Esto es lo que hay que hacer para que los iconos se vean bien.

## Qué usa la interfaz ahora mismo

Por defecto, símbolos Unicode "universales", verificados contra la fuente que
realmente usa esta máquina (`fc-match monospace` → **Noto Sans Mono**):

| icono | glifo | para |
|---|---|---|
| favorito | `◉` U+25C9 | en la lista |
| no favorito | `○` U+25CB | en la lista |
| tiempo jugado | `▶` U+25B6 | `▶ 45s` / `▶ 12m` / `▶ 3h 20m` / `▶ nunca` |

**No se usa `★`** (U+2605): comprobando el `cmap` de Noto Sans Mono, esa fuente
**no lo tiene** (tampoco `☆` U+2606). Se veía como una caja vacía. Cualquier
símbolo nuevo que se añada a la interfaz debe comprobarse igual antes de usarse;
la regla que sigue el código es "nada en uso privado" (0xE000-0xF8FF), porque
ahí es donde viven los Nerd Font.

## Nerd Font (iconos de Font Awesome)

Hay un segundo juego de iconos, **detrás de bandera**, porque una Nerd Font es
una elección y no se puede suponer:

* `ROSADECK_NERD_FONTS=1 rosadeck-library`
* glifos: `nf-fa-star` U+F005, `nf-fa-star_o` U+F006, `nf-fa-play` U+F04B.

Si tu terminal **no** tiene una Nerd Font y activas la bandera, los tres iconos
salen como cajas. Es un intercambio explícito.

### Cómo tener una Nerd Font en kitty

Esta máquina ya tiene varias instaladas (`fc-list | grep -i nerd`): RecMono
Linear NF, M+1 NF, Noto Sans NF, BlexMono NF, JetBrainsMono Nerd Font, Ubuntu NF,
Iosevka Term NF… Pero `kitty.conf` **no fija `font_family`**, así que kitty usa
su fuente por defecto (Noto Sans, sin los glifos parcheados). Para activarlas,
añade a `~/.config/kitty/kitty.conf`:

```
font_family      JetBrainsMono Nerd Font
bold_font        JetBrainsMono Nerd Font:style=Bold
italic_font      JetBrainsMono Nerd Font:style=Italic
bold_italic_font JetBrainsMono Nerd Font:style=Bold Italic
```

y recarga la configuración (kitty: `ctrl+shift+5`, o `kitty @ load-config`).

## Cómo se ha verificado

* `fc-match -f '%{file}' monospace` dice qué fuente resuelve kitty.
* Un lector de `cmap` a pelo (`tools/fontcheck.py`, sin dependencias) comprueba si
  cada glifo está en el `cmap` de esa fuente. No se supone nada: `★` salió
  ausente, y por eso la estrella se sustituyó por `◉`/`○`.

## Glifos que la interfaz ya no usa

`╭ ╮ ╰ ╯` (esquinas de tarjeta) y `│`/`─` de los marcos: las tarjetas ya no
tienen marco. Siguen en el código por si acaso, marcados como `#[allow(dead_code)]`.