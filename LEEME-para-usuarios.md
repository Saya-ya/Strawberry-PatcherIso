# StrawberryPatcher

Parcheador de la traducción de **Strawberry Panic!** (PS2).

Este programa **no crea** el juego: aplica la traducción a una copia de tu ISO
original y genera una ISO nueva y parcheada. Tus archivos originales no se
tocan nunca.

---

## 1. Qué necesitas

| Archivo | De dónde sale |
|---|---|
| `StrawPatcher.exe` | Este paquete |
| `translation_manager.db` | Este paquete (la traducción) |
| `texturas/` | Este paquete (carpeta con `manifest.json` y `uploads/`) |
| Tu ISO original japonesa | La tuya |
| `Data.bin` y `SLPS_256.11` | Se extraen de tu ISO original |

---

## 2. Cómo organizar los archivos

Crea una carpeta y pon dentro el `.exe` y las carpetas. **Quedan todas al lado
del ejecutable**, no dentro de `Program Files` ni en una carpeta del escritorio
con espacios raros.

```
StrawPatcher/
├── StrawberryPatcher.exe
├── translation_manager.db
│
├── originales/                  ← TUS ARCHIVOS ORIGINALES
│   ├── Strawberry_patched.iso   ← tu ISO japonesa (cualquier nombre .iso)
│   ├── Data.bin                 ← sale de la ISO
│   └── SLPS_256.11              ← sale de la ISO
│
└── texturas/                    ← TEXTURAS TRADUCIDAS (incluido)
    ├── manifest.json
    └── uploads/                 ← los PNG de las texturas
```

### Qué va en cada carpeta

**`originales/`** — solo tus archivos, nunca los modifiques:

- **`Strawberry_patched.iso`** — tu ISO original en japonés. El programa busca
  cualquier archivo `.iso` dentro de esta carpeta, así que el nombre da igual.
- **`Data.bin`** — está dentro de la ISO. Se puede extraer con 7-Zip, WinRAR o
  cualquier herramienta que abra ISO de PS2.
- **`SLPS_256.11`** — igual que el anterior, también está dentro de la ISO.
  Es el ejecutable del juego, y es donde viven algunos textos.

> ¿No sabes dónde están? Abre tu ISO con 7-Zip como si fuera un `.zip`. Busca
> `DATA.BIN` y `SLPS_256.11`, y cópialos a `originales/`.

**`texturas/`** — viene listo en el paquete, **no la modifiques**:

- **`manifest.json`** — la lista de texturas a parchear.
- **`uploads/`** — los PNG con las imágenes ya traducidas.

Si borras, renuevas o mueves un PNG de `uploads/`, el parcheo fallará.

> **Necesitas `manifest.json` Y `uploads/`.** El `.json` solo dice *qué*
> cambiar; los PNG son los cambios en sí.

> También vale con los PNG **sueltos en la misma carpeta** que el
> `manifest.json`, sin la subcarpeta `uploads/`. Pero si usas una subcarpeta,
> tiene que llamarse **`uploads`** (con otro nombre el parcheo falla).

---

## 3. Cómo parchear

1. Abre la carpeta `StrawPatcher/`.
2. Haz doble clic en **`StrawPatcher.exe`**.
3. Revisa los 4 campos. Deberían apuntar a:

   | Campo | Qué hay que señalar |
   |---|---|
   | **Carpeta originales** | La carpeta `originales/` |
   | **Base de traducción (.db)** | El archivo `translation_manager.db` |
   | **Carpeta de texturas** | La carpeta `texturas/` (la que contiene `manifest.json`) |
   | **ISO de salida** | `output/Strawberry_Patched.iso` |

   En **"Carpeta de texturas"** señalas **la carpeta**, no el archivo
   `manifest.json` por separado: el programa busca los PNG dentro de esa
   carpeta por su cuenta.

   Si dejaste todo como viene, no tienes que tocar nada.
4. Pulsa el botón verde **▶ PARCHEAR ISO**.
5. Espera. Tarda entre **1 y 2 minutos** y el log va mostrando el progreso.
6. Cuando aparezca *"Parcheo terminado correctamente"*, tu ISO está en:

```
StrawPatcher/output/Strawberry_Patched.iso
```

> Las rutas de los campos siempre se interpretan **desde la carpeta donde está
> el `.exe`**, así que funciona aunque lo abras desde otro sitio. Si mueves el
> `.exe`, mueve también las carpetas.

### Ajustar el rendimiento

Por defecto usa varios núcleos. Si el equipo va lento o quieres usar menos CPU,
desmarca **"Ajuste automático de hilos"** y mueve el deslizador. Con 1 hilo
funciona igual, solo que más despacio.

---

## 4. Al terminar

Copia `output/Strawberry_Patched.iso` a tu USB y úsala en tu emulador (PCSX2,
RPCS3, etc.).

- Es una ISO **independiente**: no necesita los archivos sueltos ni la
  carpeta `originales/`.
- Puedes renombrarla como quieras.
- Tu ISO japonesa original sigue intacta, por si quieres volver atrás.

---

## 5. Problemas frecuentes

**"No se encontró el archivo requerido"**
Falta `Data.bin` o `SLPS_256.11` en `originales/`. Extráelos de la ISO y listo.

**"No se encontró ISO en originales"**
No hay ningún `.iso` en esa carpeta. Copia tu ISO japanese ahí.

**"Fallaron N texturas"**
A la carpeta `texturas/` le falta un PNG o el `manifest.json` está mal. Vuelve a
descargar el paquete completo. El mensaje indica exactamente qué archivo falta.

**"AVISO: no existe .../manifest.json; se omiten las texturas"**
No encontró la carpeta `texturas/`. La ISO se genera igual, pero **solo con
textos**: los textos saldrán con los glifos originales del juego y se verá
raro. Si no es lo que quieres, revisa que la carpeta `texturas/` esté al lado
del `.exe`.

**"La salida no puede sobrescribir la ISO original"**
Has puesto la ISO japonesa como destino. Elige una ruta de salida distinta,
por ejemplo `output/Strawberry_Patched.iso`.

**"No space left on device" / "Espacio insuficiente"**
El parcheo necesita unos **5 GB libres** (2 GB de archivos temporales + 3 GB de
la ISO de salida). Los temporales se crean junto a la ISO de salida, así que
asegúrate de tener espacio en esa unidad. Si usas un disco externo o una
carpeta de red, puedes usar el campo de salida para mover la generación a tu
disco principal.

**El programa no arranca**
Windows a veces bloquea ejecutables descargados:

1. Clic derecho en `StrawPatcher.exe` → **Propiedades**.
2. Abajo, marca **"Desbloquear"**.
3. **Aplicar** → **Aceptar**.

Si aparece el aviso de SmartScreen, pulsa **"Más información"** →
**"Ejecutar de todas formas"**.

---

## 6. Detalles técnicos

### Glifos e idioma

El programa lee de la base de datos el idioma de destino y aplica la
sustitución de caracteres correspondiente:

| Idioma | Qué hace |
|---|---|
| **Español (es)** | Traduce `á`, `é`, `ñ`… a los glifos cirílicos que la fuente del juego ya tiene. |
| **Inglés (en)** | No necesita nada: el inglés solo usa ASCII. |
| **Personalizado** | Usa el mapa que definiste en la herramienta de traducción. |

No tienes que elegir nada: se detecta solo desde el `.db`.

### Cuánto ocupa

| Carpeta | Tamaño |
|---|---|
| `originales/` | ≈ 4.9 GB (tu ISO, `Data.bin` y `SLPS_256.11`) |
| `texturas/` | ≈ 2 MB |
| `output/` | ≈ 3 GB (la ISO parcheada) |

Los temporales se borran solos al terminar. Si el programa se cierra a la
fuerza, puede quedar una carpeta `straw-patcher-XXXX` en `output/`; puedes
borrarla sin miedo.

---

¡Gracias por usar la traducción!