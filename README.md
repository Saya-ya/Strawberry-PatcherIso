# Strawberry-PatcherIso

[English](#english) | [Español](#español)

---

## English

This is an open-source, standalone patcher for the PS2 game *"Strawberry Panic!"*. Unlike the translator-oriented repository, this patcher is designed so that **end users** can easily apply the translation patch to their own copy of the game's ISO with just a few clicks.

### For the End User (No Technical Knowledge Required)

If you just want to apply the translation, **you don't need to install Python or use the command line**.

1. Download the `.zip` file from the **Releases** section on this GitHub page.
2. Extract the file.
3. Double-click the `StrawPatcher.exe` file.
4. In the window that opens, select:
   - Your **Original Japanese ISO**.
   - The **Translation Database file (`.db`)**.
   - The `Data.bin` and `SLPS_256.11` files (if prompted or not bundled).
5. Click **PATCH** and wait for it to finish. That's it! Your patched ISO will be generated in the `output` folder.

---

### For Developers (Source Code)

If you want to modify the patcher or build it yourself:

#### Requirements
- Python 3.8 or higher.
- Standard Python libraries (the project mostly relies on built-in libraries like `tkinter`, `sqlite3`, `csv`, `multiprocessing`).
- Optional: `pyinstaller` to build the `.exe`.

#### Running from Source
To start the GUI:
```bash
python gui.py
```

To use the command-line version without a GUI:
```bash
python main.py --iso your_original_game.iso --db translation_db.db
```

#### Build the Executable (.exe)
```bash
pip install pyinstaller
pyinstaller StrawPatcher.spec
```
The `StrawPatcher.exe` file will be generated in the `dist/` folder.

---
---

## Español

Este es un parcheador "standalone" de código abierto para el juego de PS2 *"Strawberry Panic!"*. A diferencia del repositorio para traductores, este parcheador está diseñado para que los **usuarios finales** apliquen la traducción directamente a su propia copia de la ISO del juego con un par de clics.

### Para el Usuario Final (No requiere conocimientos técnicos)

Si solo quieres aplicar la traducción, **no necesitas instalar Python ni usar la línea de comandos**.

1. Descarga el archivo `.zip` desde la sección de **Releases** en este GitHub.
2. Descomprime el archivo.
3. Haz doble clic en el archivo `StrawPatcher.exe`.
4. En la ventana que se abre, selecciona:
   - Tu **ISO original** en japonés.
   - El archivo de la **base de datos de traducción (`.db`)**.
   - Los archivos `Data.bin` y `SLPS_256.11` (si se te solicitan o no están integrados).
5. Dale a **PARCHEAR** y espera a que termine. ¡Y listo! Tendrás tu ISO parcheada en la carpeta `output`.

---

### Para Desarrolladores (Código Fuente)

Si quieres modificar el parcheador o compilarlo por ti mismo:

#### Requisitos
- Python 3.8 o superior.
- Librerías estándar de Python (el proyecto usa mayoritariamente librerías integradas como `tkinter`, `sqlite3`, `csv`, `multiprocessing`).
- Opcional: `pyinstaller` para construir el `.exe`.

#### Ejecutar desde el código fuente
Para iniciar la interfaz gráfica:
```bash
python gui.py
```

Para usar la versión de línea de comandos sin interfaz:
```bash
python main.py --iso tu_juego_original.iso --db base_traduccion.db
```

#### Compilar el ejecutable (.exe)
```bash
pip install pyinstaller
pyinstaller StrawPatcher.spec
```
El archivo `StrawPatcher.exe` aparecerá en la carpeta `dist/`.
