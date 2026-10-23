# Strawberry-PatcherIso

[English](#english) | [Español](#español)

---

## English

This is an open-source, standalone patcher for the PS2 game *"Strawberry Panic!"*. Unlike the translator-oriented repository, this patcher is designed so that **end users** can easily apply the translation patch to their own copy of the game's ISO with just a few clicks.


### Tutorial en español aqui

[¿Como instalar el parche en el iso?](https://www.canva.com/design/DAHOrUyseRQ/c5uiigaEZIFXx09FVrh0EA/view?utm_content=DAHOrUyseRQ&utm_campaign=designshare&utm_medium=link2&utm_source=uniquelinks&utlId=h88ddc90a5c)

### For the End User (No Technical Knowledge Required)

> 👉 **See [LEEME-para-usuarios.md](LEEME-para-usuarios.md)** for the folder layout and step-by-step instructions to ship with the `.exe`.

If you just want to apply the translation, you don't need to install development tools or use the command line.

1. Download `StrawPatcher.exe` from the **Releases** section on this GitHub page.
2. Download and extract the translation archive (`StrawBerry-Spanish.rar`) in the same folder.
3. Double-click the `StrawPatcher.exe` file.
4. Arrange the files next to the executable like this:

```
StrawPatcher/
├── StrawberryPatcher.exe
├── translation_manager.db     ← the translation database
├── originales/                ← your original Japanese ISO + Data.bin + SLPS_256.11
│   ├── Strawberry_patched.iso
│   ├── Data.bin
│   └── SLPS_256.11
└── Texturas/                  ← the texture pack
    ├── manifest.json
    └── uploads/               ← the edited PNGs
```

5. Click **PATCH** and wait for it to finish. That's it! Your patched ISO will be generated in the `output` folder.

> You need your own copy of the original Japanese game. The download does not include it.

### Language & Glyph Mapping

The patcher automatically reads the **target language** from the translation database (`.db`) and applies the correct **glyph substitution** during the patching process.

| Target Language | Behavior |
|---|---|
| **Spanish (es)** | Uses the hardcoded Spanish→Cyrillic map (`á→Г`, `é→Д`, `ñ→И`, etc.). The game's Japanese font displays Cyrillic glyphs, which are visually replaced by Spanish characters via PCSX2 texture injection. |
| **English (en)** | No glyph mapping needed. English uses only ASCII characters present in the game's font. |
| **Custom** | Loads a user-defined glyph map from the database (`settings.custom_glyph_map`). The webapp allows configuring this mapping for any language whose characters fit in Latin-1 (e.g. Polish `ą→Г, ć→Д, ę→Е, ł→Ж`). PCSX2 texture replacement must be configured accordingly. |

The detected language and active glyph mapping are applied automatically from the database during patching.

---

### For Developers (Source Code)

If you want to modify the patcher or build it yourself:

#### Rust patcher (native GUI and translation pipeline)

The Rust command-line patcher uses the current `StrawTraduccion` core: it reads translations and language settings from the SQLite database, rebuilds scripts and the ELF from the originals, applies optional texture patches, then creates the output ISO. Keep the original ISO, `Data.bin` and `SLPS_256.11` in `originales/`; keep `manifest.json` and the PNG files it references in `texturas/`.

```bash
cargo run --release -p straw-patcher
```

The GUI lets you choose `originales/`, the `.db`, the texture folder and the output ISO. The texture step is skipped when that folder has no `manifest.json`. The folder name is free: the PNGs can sit in `uploads/` next to the manifest or loose in the same folder.

To build the Windows x64 executable, run `./build_windows.ps1` on Windows with the Rust MSVC toolchain installed; it writes `dist/StrawPatcher.exe`. The executable and patching pipeline are Rust and do not require Python. The `x86_64-pc-windows-gnu` target also produces a standalone `.exe` with no extra DLLs, so it can be cross-compiled from Linux.

#### Threads and work folder

Patching uses several cores by default. In the GUI you can enable **"Ajuste automático de hilos"** (uses the available cores, up to 8) or set a number manually.

From the command line:

- `--workers N` — number of threads (`0` = automatic detection, the default). Scripts and textures are patched in parallel; the result is **byte-for-byte identical** to the sequential run.
- `--work-dir PATH` — where the intermediate files are created (`Data_patched.bin` takes about 2 GB). It defaults to the output ISO's folder rather than the system temp folder, which is often in memory and can be too small.

```bash
cargo run --release -p straw-patcher -- --cli --workers 4 --work-dir D:/straw_work
```

---
---

## Español

Este es un parcheador "standalone" de código abierto para el juego de PS2 *"Strawberry Panic!"*. A diferencia del repositorio para traductores, este parcheador está diseñado para que los **usuarios finales** apliquen la traducción directamente a su propia copia de la ISO del juego con un par de clics.

### Para el Usuario Final (No requiere conocimientos técnicos)

> 👉 **Mira [LEEME-para-usuarios.md](LEEME-para-usuarios.md)** para la estructura de carpetas y las instrucciones paso a paso que acompañan al `.exe`.

Si solo quieres aplicar la traducción, no necesitas instalar herramientas de desarrollo ni usar la línea de comandos.

1. Descarga `StrawPatcher.exe` desde la sección de **Releases** en este GitHub.
2. Descarga y descomprime el archivo de traducción (`StrawBerry-Spanish.rar`) en la misma carpeta.
3. Haz doble clic en el archivo `StrawPatcher.exe`.
4. Coloca los archivos junto al ejecutable así:

```
StrawPatcher/
├── StrawberryPatcher.exe
├── translation_manager.db     ← la base de datos de traducción
├── originales/                ← tu ISO original japonesa + Data.bin + SLPS_256.11
│   ├── Strawberry_patched.iso
│   ├── Data.bin
│   └── SLPS_256.11
└── Texturas/                  ← el paquete de texturas
    ├── manifest.json
    └── uploads/               ← los PNG editados
```

5. Dale a **PARCHEAR** y espera a que termine. ¡Y listo! Tendrás tu ISO parcheada en la carpeta `output`.

> Necesitas tu propia copia del juego original en japonés. La descarga no lo incluye.

### Idioma y Mapeo de Glifos

El parcheador lee automáticamente el **idioma de traducción** desde la base de datos (`.db`) y aplica la **sustitución de glifos** correcta durante el proceso de parcheo.

| Idioma de destino | Comportamiento |
|---|---|
| **Español (es)** | Usa el mapa fijo español→cirílico (`á→Г`, `é→Д`, `ñ→И`, etc.). La fuente japonesa del juego muestra glifos cirílicos, que son reemplazados visualmente por caracteres españoles mediante inyección de texturas en PCSX2. |
| **Inglés (en)** | No necesita mapeo de glifos. El inglés usa solo caracteres ASCII presentes en la fuente del juego. |
| **Personalizado (custom)** | Carga un mapa de glifos definido por el usuario desde la base de datos (`settings.custom_glyph_map`). La webapp permite configurar este mapeo para cualquier idioma cuyos caracteres quepan en Latin-1 (ej. polaco: `ą→Г, ć→Д, ę→Е, ł→Ж`). El reemplazo de texturas en PCSX2 debe configurarse de forma correspondiente. |

El parcheador aplica automáticamente el idioma y el mapeo de glifos configurados en la base de datos.

---

### Para Desarrolladores (Código Fuente)

Si quieres modificar el parcheador o compilarlo por ti mismo:

#### Parcheador Rust (interfaz gráfica y pipeline de traducción)

El parcheador Rust reutiliza el núcleo actual de `StrawTraduccion`: lee traducciones e idioma desde la base SQLite, reconstruye scripts y ELF desde los originales, aplica opcionalmente los parches de texturas y genera la ISO. Coloca la ISO original, `Data.bin` y `SLPS_256.11` en `originales/`; deja `manifest.json` y los PNG referenciados en `texturas/`.

```bash
cargo run --release -p straw-patcher
```

La interfaz permite elegir `originales/`, la `.db`, la carpeta de texturas y la ISO de salida. Si esa carpeta no tiene `manifest.json`, se omite esa etapa. El nombre de la carpeta es libre: los PNG pueden estar en `uploads/` junto al manifest o sueltos en la misma carpeta.

Para compilar el ejecutable Windows x64, ejecuta `./build_windows.ps1` en Windows con Rust MSVC instalado; genera `dist/StrawPatcher.exe`. El ejecutable y el proceso de parcheo están hechos en Rust y no requieren Python. El target `x86_64-pc-windows-gnu` también genera un `.exe` autónomo sin DLLs adicionales, así que se puede compilar desde Linux.

#### Hilos y carpeta de trabajo

El parcheo usa varios núcleos por defecto. En la interfaz gráfica puedes activar **"Ajuste automático de hilos"** (usa los núcleos disponibles, hasta 8) o fijar un número manualmente.

Desde la consola:

- `--workers N` — número de hilos (`0` = detección automática, por defecto). Los scripts y las texturas se parchean en paralelo; el resultado es **idéntico byte a byte** al parcheo secuencial.
- `--work-dir RUTA` — dónde crear los archivos intermedios (`Data_patched.bin` ocupa unos 2 GB). Por defecto se usa la carpeta de la ISO de salida, no la carpeta temporal del sistema, que suele estar en memoria y puede ser demasiado pequeña.

```bash
cargo run --release -p straw-patcher -- --cli --workers 4 --work-dir D:/straw_work
```
