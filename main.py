"""
StrawPatcher — Strawberry Panic! (PS2) standalone patcher

Double click or CLI:
  python main.py
  python main.py --iso my.iso --db my.db -o patched.iso
"""

import argparse
import csv
import os
import shutil
import sqlite3
import struct
import sys
import time
import traceback
from concurrent.futures import ProcessPoolExecutor, as_completed
from datetime import datetime
from pathlib import Path
import sys

_core_dir = Path(__file__).resolve().parent / "core"
if str(_core_dir) not in sys.path:
    sys.path.insert(0, str(_core_dir))

import lz77, datafat
from script_rebuilder import load_csv_rows, rebuild_local_slack
from extract_all import extract_entry

BASE_DIR = Path.cwd()
DEFAULT_INPUT = BASE_DIR / "input"
DEFAULT_WORK = BASE_DIR / "work"
DEFAULT_OUTPUT = BASE_DIR / "output"



class Logger:
    def __init__(self, log_path: Path = None, console: bool = True):
        self._console = console
        self._file = None
        if log_path:
            log_path.parent.mkdir(parents=True, exist_ok=True)
            self._file = open(log_path, "w", encoding="utf-8")

    def log(self, msg: str):
        ts = datetime.now().strftime("%H:%M:%S")
        line = f"[{ts}] {msg}"
        if self._console:
            print(line)
        if self._file:
            self._file.write(line + "\n")
            self._file.flush()

    def close(self):
        if self._file:
            self._file.close()



def _compress_worker(args: tuple) -> dict:
    script_id, csv_path, dec_dir = args
    dec_path = Path(dec_dir) / f"ID_{script_id:05d}.dec"
    if not dec_path.exists():
        return {"success": False, "script_id": script_id, "error": ".dec missing"}

    dec_data = dec_path.read_bytes()
    rows = load_csv_rows(Path(csv_path), script_id)
    if not rows:
        return {"success": False, "script_id": script_id, "error": "no data"}

    try:
        rebuilt, report = rebuild_local_slack(dec_data, rows)
    except Exception as e:
        return {"success": False, "script_id": script_id, "error": str(e)}

    if report.get("needs_shift"):
        segs = report["needs_shift"]
        detail = "; ".join(f"0x{s['start']:X}:{s['required_bytes']}>{s['capacity_bytes']}" for s in segs[:5])
        return {"success": False, "script_id": script_id, "error": f"needs_shift: {detail}"}

    comp_data = lz77.compress(rebuilt)
    redec = lz77.decompress(comp_data)
    diffs = sum(a != b for a, b in zip(rebuilt, redec))
    if diffs > 0 or len(rebuilt) != len(redec):
        return {"success": False, "script_id": script_id, "error": f"round-trip: {diffs} diffs"}

    return {"success": True, "script_id": script_id, "comp_data": comp_data}



def patch(
    iso_path: Path,
    db_path: Path,
    output_path: Path,
    input_dir: Path = DEFAULT_INPUT,
    work_dir: Path = DEFAULT_WORK,
    workers: int = None,
    log: Logger = None,
) -> bool:
    if log is None:
        log = Logger()
    if workers is None:
        workers = min(os.cpu_count() or 4, 12)

    t_start = time.time()
    log.log("=" * 55)
    log.log("  StrawPatcher — Strawberry Panic! (PS2)")
    log.log(f"  {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    log.log(f"  Workers: {workers} processes")
    log.log("=" * 55)

    work_dir.mkdir(parents=True, exist_ok=True)
    output_path.parent.mkdir(parents=True, exist_ok=True)

    csv_path = work_dir / "dialogo.csv"
    bin_patched = work_dir / "Data_patched.bin"
    elf_patched = work_dir / "SLPS_256.11_translated"
    dec_dir = work_dir / "scripts_extraidos"

    data_bin_src = input_dir / "Data.bin"
    elf_src = input_dir / "SLPS_256.11"

    errors = []
    for f, label in [(iso_path, "ISO"), (db_path, "DB"), (data_bin_src, "Data.bin"), (elf_src, "SLPS_256.11")]:
        if not f.exists():
            errors.append(f"  MISSING: {label} ({f})")
    if errors:
        log.log("ERROR: Required files not found:")
        for e in errors:
            log.log(e)
        log.log("\nPlace the following in the 'input/' folder:")
        log.log("  - Your original game ISO")
        log.log("  - Data.bin (extracted from ISO)")
        log.log("  - SLPS_256.11 (extracted from ISO)")
        log.log("  - translation_manager.db (translation database)")
        return False

    try:
        log.log("\n[1/5] Exporting translations...")
        t = time.time()
        conn = sqlite3.connect(str(db_path))
        conn.row_factory = sqlite3.Row
        rows = conn.execute("""
            SELECT source, script_id, byte_offset, original_text, translated_text
            FROM text_entries
            WHERE is_translated = 1 AND translated_text != ''
            ORDER BY script_id, section_id, section_order
        """).fetchall()
        with open(csv_path, "w", encoding="utf-8-sig", newline="") as f:
            writer = csv.writer(f, lineterminator="\r\n")
            writer.writerow(["source", "file_id", "offset", "original_text", "translated_text"])
            for r in rows:
                src = r["source"]
                fid = str(r["script_id"]) if r["script_id"] != -1 else "ELF"
                off = f"0x{r['byte_offset']:05X}" if src == "SCRIPT" else f"0x{r['byte_offset']:06X}"
                writer.writerow([src, fid, off, r["original_text"], r["translated_text"]])
        conn.close()
        log.log(f"  {len(rows)} texts ({time.time()-t:.1f}s)")

        if len(rows) == 0:
            log.log("ERROR: No translations found.")
            return False

        script_ids = set()
        with open(csv_path, encoding="utf-8-sig", newline="") as f:
            for row in csv.DictReader(f):
                if row.get("source") == "SCRIPT" and row["file_id"].strip().isdigit():
                    script_ids.add(int(row["file_id"]))
        script_ids = sorted(script_ids)
        log.log(f"\n[2/5] Preparing {len(script_ids)} scripts...")

        dec_dir.mkdir(parents=True, exist_ok=True)
        data_bin_bytes = data_bin_src.read_bytes()
        fat_entries = datafat.read_entries(str(data_bin_src))
        fat_by_id = {e["id"]: e for e in fat_entries}
        for sid in script_ids:
            dec_path = dec_dir / f"ID_{sid:05d}.dec"
            if not dec_path.exists() and sid in fat_by_id:
                try:
                    fe = fat_by_id[sid]
                    entry = {"offset": fe["off"], "size": fe["size"], "id": fe["id"]}
                    extract_entry(data_bin_bytes, entry, work_dir)
                except Exception:
                    pass
        missing = sum(1 for sid in script_ids if not (dec_dir / f"ID_{sid:05d}.dec").exists())
        log.log(f"  Ready scripts: {len(script_ids)} (missing: {missing})")

        log.log(f"\n[3/5] Compressing ({workers} processes)...")
        t = time.time()
        compressed = {}
        ok = err = 0

        items = [(sid, csv_path, str(dec_dir)) for sid in script_ids]
        with ProcessPoolExecutor(max_workers=workers) as pool:
            futures = {pool.submit(_compress_worker, item): item[0] for item in items}
            for future in as_completed(futures):
                sid = futures[future]
                try:
                    result = future.result(timeout=300)
                except Exception as e:
                    log.log(f"  X {sid}: {e}")
                    err += 1
                    continue
                if result["success"]:
                    compressed[sid] = result["comp_data"]
                    ok += 1
                else:
                    log.log(f"  X {sid}: {result.get('error','?')[:120]}")
                    err += 1
                if (ok + err) % 20 == 0:
                    log.log(f"  ... {ok+err}/{len(script_ids)}")
        log.log(f"  OK: {ok} | Errors: {err} | {time.time()-t:.1f}s")

        log.log(f"\n[4/5] Injecting into Data.bin...")
        shutil.copy2(data_bin_src, bin_patched)

        fat_rows = datafat.read_entries(str(bin_patched))
        injected = 0
        for sid in script_ids:
            if sid not in compressed:
                continue
            target = datafat.find_row(fat_rows, sid)
            if target is None:
                log.log(f"  X {sid}: not in FAT")
                continue
            comp_data = compressed[sid]
            slot_size = datafat.slot_capacity(fat_rows, target)
            if len(comp_data) > slot_size:
                log.log(f"  X {sid}: {len(comp_data)} > {slot_size}")
                continue
            with open(bin_patched, "r+b") as f:
                f.seek(target["off"])
                f.write(comp_data)
                f.write(b"\x00" * (slot_size - len(comp_data)))
                f.seek(datafat.size_field_write_offset(target))
                f.write(struct.pack("<I", len(comp_data)))
            injected += 1
        log.log(f"  Injected: {injected}/{len(compressed)}")

        log.log("  Applying ELF translations...")
        from apply_translation import apply_translations
        apply_translations(csv_path, str(bin_patched), str(elf_src))

        log.log(f"\n[5/5] Building ISO...")
        from build_iso import build_iso
        result = build_iso(bin_patched, iso_path, output_path)
        if result is None:
            log.log("  ERROR building ISO")
            return False

        from inject_elf import inject_elf
        elf_orig = input_dir / "SLPS_256.11"
        inject_elf(str(output_path), str(elf_patched), str(elf_orig))

        total_t = time.time() - t_start
        log.log(f"\n{'='*55}")
        log.log(f"  ISO built: {output_path}")
        log.log(f"  Size: {output_path.stat().st_size//1024//1024} MB")
        log.log(f"  Time: {total_t:.0f}s ({total_t/60:.1f} min)")
        log.log(f"{'='*55}")
        return True

    except Exception as e:
        log.log(f"\nERROR: {e}")
        log.log(traceback.format_exc())
        return False



def main():
    parser = argparse.ArgumentParser(description="StrawPatcher — Strawberry Panic! (PS2)")
    parser.add_argument("--iso", type=Path, help="Original ISO")
    parser.add_argument("--db", type=Path, help="Translation .db file")
    parser.add_argument("-o", "--output", type=Path, default=DEFAULT_OUTPUT / "Strawberry_Patched.iso", help="Output ISO")
    parser.add_argument("--input-dir", type=Path, default=DEFAULT_INPUT, help="Folder containing Data.bin and SLPS_256.11")
    parser.add_argument("--workers", "-w", type=int, default=min(os.cpu_count() or 4, 12))
    args = parser.parse_args()

    iso = args.iso
    if iso is None:
        candidates = list(args.input_dir.glob("*.iso"))
        if candidates:
            iso = candidates[0]

    db = args.db
    if db is None:
        candidates = list(args.input_dir.glob("*.db"))
        if candidates:
            db = candidates[0]

    if iso is None or db is None:
        print("ERROR: ISO or DB not found.")
        print("Place your ISO and translation_manager.db in the 'input/' folder")
        print("Or use: python main.py --iso my.iso --db my.db")
        sys.exit(1)

    log = Logger()
    ok = patch(iso, db, args.output, args.input_dir, DEFAULT_WORK, args.workers, log)
    log.close()
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
