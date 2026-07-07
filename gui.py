"""
StrawPatcher GUI — Strawberry Panic! (PS2) Patcher
Graphical interface with file selection, progress, and logging.
"""

import os
import sys
import threading
import tkinter as tk
from tkinter import ttk, filedialog, messagebox
from pathlib import Path

# ── Load patcher modules ─────────────────────────────────────────────
_self_dir = Path(__file__).resolve().parent
if str(_self_dir) not in sys.path:
    sys.path.insert(0, str(_self_dir))
_core_dir = _self_dir / "core"
if str(_core_dir) not in sys.path:
    sys.path.insert(0, str(_core_dir))

from main import patch as run_patch, DEFAULT_INPUT, DEFAULT_WORK, DEFAULT_OUTPUT


class GuiLogger:
    def __init__(self, widget: tk.Text):
        self._widget = widget

    def log(self, msg: str):
        from datetime import datetime
        ts = datetime.now().strftime("%H:%M:%S")
        line = f"[{ts}] {msg}\n"
        self._widget.insert(tk.END, line)
        self._widget.see(tk.END)
        self._widget.update_idletasks()

    def close(self):
        pass


class StrawPatcherGUI:
    def __init__(self):
        self.root = tk.Tk()
        self.root.title("StrawPatcher — Strawberry Panic! (PS2)")
        self.root.geometry("700x550")
        self.root.resizable(True, True)
        self.root.configure(bg="#1a1a2e")

        style = ttk.Style()
        style.theme_use("clam")
        style.configure("TFrame", background="#1a1a2e")
        style.configure("TLabel", background="#1a1a2e", foreground="#e0e0e0", font=("Segoe UI", 10))
        style.configure("TButton", font=("Segoe UI", 10), padding=6)
        style.configure("TLabelframe", background="#1a1a2e", foreground="#e0e0e0")
        style.configure("TLabelframe.Label", background="#1a1a2e", foreground="#e0a0ff", font=("Segoe UI", 10, "bold"))
        style.configure("TEntry", font=("Segoe UI", 10))
        style.configure("TProgressbar", thickness=20)

        self._building = False

        header = ttk.Frame(self.root)
        header.pack(fill=tk.X, padx=15, pady=(15, 5))
        ttk.Label(header, text="StrawPatcher", font=("Segoe UI", 18, "bold"),
                  foreground="#ff9ff3").pack(side=tk.LEFT)
        ttk.Label(header, text="Strawberry Panic! (PS2) · Translation Patcher",
                  font=("Segoe UI", 9), foreground="#888").pack(side=tk.LEFT, padx=12)

        files_frame = ttk.LabelFrame(self.root, text=" Input Files ", padding=10)
        files_frame.pack(fill=tk.X, padx=15, pady=5)

        self._entries = {}
        fields = [
            ("Original ISO:", "iso", "*.iso", "ISO Files (*.iso)"),
            ("Database (.db):", "db", "*.db", "SQLite DB (*.db)"),
            ("Data.bin:", "databin", "Data.bin", "Data.bin"),
            ("SLPS_256.11:", "elf", "SLPS_256.11", "SLPS_256.11"),
        ]

        for i, (label, key, pat, ft) in enumerate(fields):
            ttk.Label(files_frame, text=label).grid(row=i, column=0, sticky=tk.W, pady=3)
            entry = ttk.Entry(files_frame, width=55)
            entry.grid(row=i, column=1, padx=(5, 5), pady=3, sticky=tk.EW)
            btn = ttk.Button(files_frame, text="...", width=3,
                             command=lambda k=key, e=entry, p=pat, f=ft: self._browse(k, e, p, f))
            btn.grid(row=i, column=2, pady=3)
            self._entries[key] = entry

        auto_btn = ttk.Button(files_frame, text="Auto-detect from input/ folder",
                              command=self._auto_detect)
        auto_btn.grid(row=len(fields), column=0, columnspan=3, pady=(8, 0), sticky=tk.W)

        files_frame.columnconfigure(1, weight=1)

        opts_frame = ttk.LabelFrame(self.root, text=" Options ", padding=10)
        opts_frame.pack(fill=tk.X, padx=15, pady=5)

        ttk.Label(opts_frame, text="Workers:").grid(row=0, column=0, sticky=tk.W)
        self._workers = tk.IntVar(value=min(os.cpu_count() or 4, 12))
        ttk.Spinbox(opts_frame, from_=1, to=24, textvariable=self._workers,
                    width=5).grid(row=0, column=1, sticky=tk.W, padx=5)
        ttk.Label(opts_frame, text="(parallel processes, more = faster but uses more RAM)").grid(
            row=0, column=2, sticky=tk.W, padx=5)

        ttk.Label(opts_frame, text="Output:").grid(row=1, column=0, sticky=tk.W, pady=(5, 0))
        self._out_entry = ttk.Entry(opts_frame, width=55)
        self._out_entry.grid(row=1, column=1, padx=5, pady=(5, 0), sticky=tk.EW)
        self._out_entry.insert(0, str(DEFAULT_OUTPUT / "Strawberry_Patched.iso"))
        ttk.Button(opts_frame, text="...", width=3,
                   command=lambda: self._browse_file(self._out_entry, "*.iso", "ISO (*.iso)")
                   ).grid(row=1, column=2, pady=(5, 0))

        opts_frame.columnconfigure(1, weight=1)

        btn_frame = ttk.Frame(self.root)
        btn_frame.pack(fill=tk.X, padx=15, pady=10)

        self._build_btn = ttk.Button(btn_frame, text="▶ PATCH", command=self._build)
        self._build_btn.pack(side=tk.LEFT, ipadx=20, ipady=5)

        self._cancel_btn = ttk.Button(btn_frame, text="✕ Cancel", command=self._cancel,
                                      state=tk.DISABLED)
        self._cancel_btn.pack(side=tk.LEFT, padx=10)

        self._progress = ttk.Progressbar(self.root, mode="indeterminate")
        self._progress.pack(fill=tk.X, padx=15, pady=(0, 5))

        log_frame = ttk.LabelFrame(self.root, text=" Log ", padding=5)
        log_frame.pack(fill=tk.BOTH, expand=True, padx=15, pady=(0, 15))

        self._log = tk.Text(log_frame, height=12, bg="#0d0d1a", fg="#c0ffc0",
                            font=("Consolas", 9), wrap=tk.WORD, state=tk.NORMAL)
        self._log.pack(fill=tk.BOTH, expand=True)

        scrollbar = ttk.Scrollbar(self._log, command=self._log.yview)
        scrollbar.pack(side=tk.RIGHT, fill=tk.Y)
        self._log.configure(yscrollcommand=scrollbar.set)


    def _browse(self, key: str, entry: tk.Entry, pattern: str, filetype: str):
        if "/" in pattern or "\\" in pattern or pattern.startswith("*"):
            path = filedialog.askopenfilename(filetypes=[(filetype, pattern)])
        else:
            path = filedialog.askopenfilename(filetypes=[(filetype, pattern)],
                                              initialfile=pattern)
        if path:
            entry.delete(0, tk.END)
            entry.insert(0, path)

    def _browse_file(self, entry: tk.Entry, pattern: str, filetype: str):
        path = filedialog.asksaveasfilename(filetypes=[(filetype, pattern)],
                                            defaultextension=".iso")
        if path:
            entry.delete(0, tk.END)
            entry.insert(0, path)

    def _auto_detect(self):
        input_dir = DEFAULT_INPUT
        for key, fname in [("iso", "*.iso"), ("db", "*.db"), ("databin", "Data.bin"), ("elf", "SLPS_256.11")]:
            entry = self._entries[key]
            candidates = list(input_dir.glob(fname))
            if candidates:
                entry.delete(0, tk.END)
                entry.insert(0, str(candidates[0]))
        self._log.insert(tk.END, "[INFO] Files auto-detected from input/ folder\n")


    def _build(self):
        if self._building:
            return

        iso = Path(self._entries["iso"].get())
        db = Path(self._entries["db"].get())
        databin = Path(self._entries["databin"].get())
        elf = Path(self._entries["elf"].get())
        output = Path(self._out_entry.get())

        missing = []
        for f, label in [(iso, "ISO"), (db, "DB"), (databin, "Data.bin"), (elf, "SLPS_256.11")]:
            if not f.exists():
                missing.append(f"{label}: {f}")
        if missing:
            messagebox.showerror("Missing Files", "\n".join(missing))
            return

        input_dir = DEFAULT_INPUT
        input_dir.mkdir(parents=True, exist_ok=True)
        import shutil
        for src, name in [(databin, "Data.bin"), (elf, "SLPS_256.11")]:
            dst = input_dir / name
            if not dst.exists():
                shutil.copy2(src, dst)

        self._building = True
        self._build_btn.configure(state=tk.DISABLED)
        self._cancel_btn.configure(state=tk.NORMAL)
        self._progress.start(10)
        self._log.delete("1.0", tk.END)

        logger = GuiLogger(self._log)
        logger.log("Starting patch process...")

        workers = self._workers.get()

        def _run():
            try:
                ok = run_patch(
                    iso_path=iso,
                    db_path=db,
                    output_path=output,
                    input_dir=input_dir,
                    work_dir=DEFAULT_WORK,
                    workers=workers,
                    log=logger,
                )
                if ok:
                    logger.log(f"\n✓ ISO generated: {output}")
                else:
                    logger.log("\n✗ Patch failed. Check the log.")
            except Exception as e:
                logger.log(f"\nERROR: {e}")
                import traceback
                logger.log(traceback.format_exc())
            finally:
                self._done()

        threading.Thread(target=_run, daemon=True).start()

    def _cancel(self):
        self._log.insert(tk.END, "[!] Cancelled by user\n")
        self._done()

    def _done(self):
        self._building = False
        self._progress.stop()
        self._build_btn.configure(state=tk.NORMAL)
        self._cancel_btn.configure(state=tk.DISABLED)

    def run(self):
        self.root.mainloop()


if __name__ == "__main__":
    StrawPatcherGUI().run()
