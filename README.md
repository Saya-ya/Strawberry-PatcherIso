# Strawberry-PatcherIso

[English](#english) | [Español](#español)

---

## English

This is an open-source, standalone patcher for the PS2 game *"Strawberry Panic!"*. Unlike the translator-oriented repository, this patcher is designed so that **end users** can easily apply the translation patch to their own copy of the game's ISO with just a few clicks.


### Tutorial en español aqui

[¿Como instalar el parche en el iso?](https://www.canva.com/design/DAHOrUyseRQ/c5uiigaEZIFXx09FVrh0EA/view?utm_content=DAHOrUyseRQ&utm_campaign=designshare&utm_medium=link2&utm_source=uniquelinks&utlId=h88ddc90a5c)

### For the End User (No Technical Knowledge Required)

If you just want to apply the translation, you don't need to install development tools or use the command line.

1. Download the `.zip` file from the **Releases** section on this GitHub page.
2. Extract the file.
3. Double-click the `StrawPatcher.exe` file.
4. In the window that opens, select:
   - Your **Original Japanese ISO**.
   - The **Translation Database file (`.db`)**.
   - The `Data.bin` and `SLPS_256.11` files (if prompted or not bundled).
  5. Click **PATCH** and wait for it to finish. That's it! Your patched ISO will be generated in the `output` folder.

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

The GUI lets you choose `originales/`, the `.db`, `texturas/` and the output ISO. The texture step is skipped when `texturas/manifest.json` is absent. To build the Windows x64 executable, run `./build_windows.ps1` on Windows with the Rust MSVC toolchain installed; it writes `dist/StrawPatcher.exe`. The executable and patching pipeline are Rust and do not require Python.

#### Hilos y carpeta de trabajo

El parcheo usa varios núcleos por defecto. En la interfaz gráfica puedes activar **"Ajuste automático de hilos"** (usa los núcleos disponibles, hasta 8) o fijar un número manualmente.

Desde la consola:

- `--workers N` — número de hilos (`0` = detección automática, por defecto). Los scripts y las texturas se parchean en paralelo; el resultado es **idéntico byte a byte** al parcheo secuencial.
- `--work-dir RUTA` — dónde crear los archivos intermedios (`Data_patched.bin` ocupa unos 2 GB). Por defecto se usa la carpeta de la ISO de salida, no la carpeta temporal del sistema, que suele estar en memoria y puede ser demasiado pequeña.

```bash
cargo run --release -p straw-patcher -- --cli --workers 4 --work-dir D:/straw_work
```

---
---

## Español

Este es un parcheador "standalone" de código abierto para el juego de PS2 *"Strawberry Panic!"*. A diferencia del repositorio para traductores, este parcheador está diseñado para que los **usuarios finales** apliquen la traducción directamente a su propia copia de la ISO del juego con un par de clics.

### Para el Usuario Final (No requiere conocimientos técnicos)

Si solo quieres aplicar la traducción, no necesitas instalar herramientas de desarrollo ni usar la línea de comandos.

1. Descarga el archivo `.zip` desde la sección de **Releases** en este GitHub.
2. Descomprime el archivo.
3. Haz doble clic en el archivo `StrawPatcher.exe`.
4. En la ventana que se abre, selecciona:
   - Tu **ISO original** en japonés.
   - El archivo de la **base de datos de traducción (`.db`)**.
   - Los archivos `Data.bin` y `SLPS_256.11` (si se te solicitan o no están integrados).
  5. Dale a **PARCHEAR** y espera a que termine. ¡Y listo! Tendrás tu ISO parcheada en la carpeta `output`.

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

La interfaz permite elegir `originales/`, la `.db`, `texturas/` y la ISO de salida. Si no existe `texturas/manifest.json`, se omite esa etapa. Para compilar el ejecutable Windows x64, ejecuta `./build_windows.ps1` en Windows con Rust MSVC instalado; genera `dist/StrawPatcher.exe`. El ejecutable y el proceso de parcheo están hechos en Rust y no requieren Python.

#### Hilos y carpeta de trabajo

El parcheo usa varios núcleos por defecto. En la interfaz gráfica puedes activar **"Ajuste automático de hilos"** (usa los núcleos disponibles, hasta 8) o fijar un número manualmente.

Desde la consola:

- `--workers N` — número de hilos (`0` = detección automática, por defecto). Los scripts y las texturas se parchean en paralelo; el resultado es **idéntico byte a byte** al parcheo secuencial.
- `--work-dir RUTA` — dónde crear los archivos intermedios (`Data_patched.bin` ocupa unos 2 GB). Por defecto se usa la carpeta de la ISO de salida, no la carpeta temporal del sistema, que suele estar en memoria y puede ser demasiado pequeña.

```bash
cargo run --release -p straw-patcher -- --cli --workers 4 --work-dir D:/straw_work
```
