//! Entry point. The same flags drive three shapes of run: a printed table
//! (`--list-devices`, `--list-presets`), a headless CSV (`--csv`), and the GUI
//! window, whose offline mode (`--wav` without `--csv`) runs the whole analysis
//! before the first paint so the window opens on the settled note.

use std::path::Path;

use tuna::app::TunaApp;
use tuna::cli::{self, Offline};
use tuna::model::config::AppConfig;
use tuna::model::presets::tuning_for;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let f = cli::parse(&args);

    if f.help {
        print!("{}", cli::USAGE);
        return;
    }
    if !f.errors.is_empty() {
        for e in &f.errors {
            eprintln!("tuna: {e}");
        }
        eprintln!("{}", cli::USAGE);
        std::process::exit(2);
    }
    if f.list_devices {
        print!("{}", cli::list_devices_text());
        return;
    }
    if f.list_presets {
        print!("{}", cli::list_presets_text());
        return;
    }

    let (mut cfg, note) = AppConfig::load();
    f.apply(&mut cfg);
    let tuning = tuning_for(&cfg.preset, &cfg.custom_tuning);

    if let Some(path) = &f.wav {
        let off = match offline(path, &cfg, &tuning) {
            Ok(o) => o,
            Err(e) => die(&e),
        };
        if f.csv {
            println!("{}", cli::CSV_HEADER);
            for r in &off.readings {
                println!("{}", cli::csv_row(r));
            }
            return;
        }
        eprintln!(
            "{} Hz · {} ch · {} · {:.2} s · {} analysis rows",
            off.fs,
            off.channels,
            off.format,
            off.duration_s,
            off.readings.len()
        );
        run_gui(cfg, note, Some(off));
        return;
    }

    if f.csv {
        if let Err(e) = cli::run_live_csv(&cfg, &tuning, f.seconds.unwrap_or(10.0)) {
            die(&e);
        }
        return;
    }

    run_gui(cfg, note, None);
}

fn offline(path: &Path, cfg: &AppConfig, tuning: &[u8]) -> Result<Offline, String> {
    cli::analyse_wav(path, cfg, tuning)
}

fn die(e: &str) -> ! {
    eprintln!("tuna: {e}");
    std::process::exit(1);
}

fn run_gui(cfg: AppConfig, note: Option<String>, offline: Option<Offline>) {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 680.0])
            .with_min_inner_size([780.0, 540.0])
            .with_title("Tuna"),
        // glow, not wgpu: the software fallback path is the one that has to
        // work on a machine with no particularly good driver
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let creator = Box::new(move |cc: &eframe::CreationContext<'_>| {
        Ok(Box::new(TunaApp::new(cc, cfg, note, offline)) as Box<dyn eframe::App>)
    });
    if let Err(e) = eframe::run_native("Tuna", opts, creator) {
        die(&e.to_string());
    }
}
