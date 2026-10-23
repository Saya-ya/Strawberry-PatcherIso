use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use eframe::egui;

pub struct PatcherGui {
    originals: String,
    database: String,
    textures: String,
    output: String,
    workers: usize,
    auto_threads: bool,
    log: Vec<String>,
    running: bool,
    receiver: Receiver<GuiMessage>,
    sender: Sender<GuiMessage>,
}

enum GuiMessage {
    Log(String),
    Finished(Result<(), String>),
}

impl Default for PatcherGui {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        let suggested = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
            .clamp(1, 8);
        Self {
            originals: "originales".into(),
            database: "translation_manager.db".into(),
            textures: "texturas".into(),
            output: "output/Strawberry_Patched.iso".into(),
            workers: suggested,
            auto_threads: true,
            log: vec!["Selecciona los archivos y pulsa PARCHEAR.".into()],
            running: false,
            receiver,
            sender,
        }
    }
}

impl eframe::App for PatcherGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                GuiMessage::Log(line) => self.log.push(line),
                GuiMessage::Finished(result) => {
                    self.running = false;
                    match result {
                        Ok(()) => self.log.push("Parcheo terminado correctamente.".into()),
                        Err(err) => self.log.push(format!("ERROR: {err}")),
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("StrawPatcher · Strawberry Panic! (PS2)");
            ui.label("Parcheador Rust — los originales no se modifican.");
            ui.add_space(12.0);
            path_row(
                ui,
                "Carpeta originales",
                &mut self.originals,
                PickKind::Folder,
            );
            path_row(
                ui,
                "Base de traducción (.db)",
                &mut self.database,
                PickKind::Database,
            );
            path_row(
                ui,
                "Carpeta de texturas",
                &mut self.textures,
                PickKind::Folder,
            );
            path_row(ui, "ISO de salida", &mut self.output, PickKind::Output);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.auto_threads, "Ajuste automático de hilos");
                if !self.auto_threads {
                    ui.label("Hilos:");
                    let mut workers = self.workers as f32;
                    if ui
                        .add(egui::Slider::new(&mut workers, 1.0..=32.0).text(""))
                        .changed()
                    {
                        self.workers = workers.round() as usize;
                    }
                    ui.label(self.workers.to_string());
                }
            });
            ui.add_space(8.0);
            if ui
                .add_enabled(!self.running, egui::Button::new("▶  PARCHEAR ISO"))
                .clicked()
            {
                self.start_patch();
            }
            ui.separator();
            ui.label("Registro");
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.log {
                        ui.monospace(line);
                    }
                });
        });
        if self.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

enum PickKind {
    Folder,
    Database,
    Output,
}

fn path_row(ui: &mut egui::Ui, label: &str, value: &mut String, kind: PickKind) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.text_edit_singleline(value);
        if ui.button("Examinar…").clicked() {
            let picked = match kind {
                PickKind::Folder => rfd::FileDialog::new().pick_folder(),
                PickKind::Database => rfd::FileDialog::new()
                    .add_filter("SQLite", &["db", "sqlite", "sqlite3"])
                    .pick_file(),
                PickKind::Output => rfd::FileDialog::new()
                    .add_filter("ISO", &["iso"])
                    .set_file_name("Strawberry_Patched.iso")
                    .save_file(),
            };
            if let Some(path) = picked {
                *value = path.display().to_string();
            }
        }
    });
}

impl PatcherGui {
    fn start_patch(&mut self) {
        let executable = match std::env::current_exe() {
            Ok(path) => path,
            Err(err) => {
                self.log
                    .push(format!("No se encontró el ejecutable: {err}"));
                return;
            }
        };
        let originals = PathBuf::from(&self.originals);
        let database = PathBuf::from(&self.database);
        let textures = PathBuf::from(&self.textures);
        let output = PathBuf::from(&self.output);
        let workers = if self.auto_threads { 0 } else { self.workers };
        let sender = self.sender.clone();
        self.running = true;
        self.log.clear();

        thread::spawn(move || {
            let result = run_child(
                &executable,
                &originals,
                &database,
                &textures,
                &output,
                workers,
                &sender,
            );
            let _ = sender.send(GuiMessage::Finished(result));
        });
    }
}

fn run_child(
    exe: &PathBuf,
    originals: &PathBuf,
    database: &PathBuf,
    textures: &PathBuf,
    output: &PathBuf,
    workers: usize,
    sender: &Sender<GuiMessage>,
) -> Result<(), String> {
    let mut child = Command::new(exe)
        .args(["--cli", "--originales"])
        .arg(originals)
        .arg("--db")
        .arg(database)
        .arg("--texturas")
        .arg(textures)
        .arg("--output")
        .arg(output)
        .arg("--workers")
        .arg(workers.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("No se pudo iniciar el parcheador: {err}"))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let out_sender = sender.clone();
    let out_thread = thread::spawn(move || forward_lines(stdout, out_sender));
    let err_sender = sender.clone();
    let err_thread = thread::spawn(move || forward_lines(stderr, err_sender));
    let status = child
        .wait()
        .map_err(|err| format!("Error esperando el parcheo: {err}"))?;
    let _ = out_thread.join();
    let _ = err_thread.join();
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "El proceso terminó con código {}",
            status.code().unwrap_or(-1)
        ))
    }
}

fn forward_lines(reader: impl std::io::Read, sender: Sender<GuiMessage>) {
    for line in BufReader::new(reader).lines().map_while(Result::ok) {
        let _ = sender.send(GuiMessage::Log(line));
    }
}
