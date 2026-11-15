#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::time::{Duration, Instant};
use eframe::egui;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use global_hotkey::hotkey::{Code, HotKey};
use serde::{Deserialize, Serialize};
use sysinfo::{ProcessesToUpdate, System};
use windows::core::w;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowLongPtrW, GWL_EXSTYLE, SetWindowLongPtrW, WS_EX_NOACTIVATE};

const PROCESS_NAME: &str = "masterduel";
const LAUNCH_DELAY: Duration = Duration::from_secs(30);
const WINDOW_TITLE: &str = "MasterDuel Secret Pack Helper";

const NAMES_RAW: &str = include_str!("../names.txt");

#[derive(Serialize, Deserialize, Default)]
struct SavedState {
    index: usize,
    word: String
}

enum Phase {
    WaitingForGame,
    Countdown(Instant),
    Running,
}

struct App {
    words: Vec<String>,
    index: usize,
    phase: Phase,

    sys: System,
    last_scan: Instant,
    clipboard: Option<arboard::Clipboard>,

    _hotkeys: GlobalHotKeyManager,
    k_confirm: u32,
    k_back: u32,
    k_copy: u32,

    no_activate_done: bool
}

fn state_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("state.json")))
        .unwrap_or_else(|| PathBuf::from("state.json"))
}

fn parse_names(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.to_lowercase().starts_with("unlocks"))
        .map(|l| l.strip_suffix("[x]").unwrap_or(l).trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()

}

impl App  {
    fn new() -> Self {
        let words = parse_names(NAMES_RAW);

        let mut index = 0;
        if let Ok(txt) = std::fs::read_to_string(state_path()) {
            if let Ok(st) = serde_json::from_str::<SavedState>(&txt) {
                if st.index < words.len() && words[st.index] == st.word {
                    index = st.index;
                } else if let Some(i) = words.iter().position(|w| *w == st.word) {
                    index = i;
                } else {
                    index = st.index.min(words.len());
                }
            }
        }

        let mgr = GlobalHotKeyManager::new().expect("hotkey manager");
        let hk_confirm = HotKey::new(None, Code::F8);
        let hk_back = HotKey::new(None, Code::F7);
        let hk_copy = HotKey::new(None, Code::F6);
        let _ = mgr.register(hk_confirm);
        let _ = mgr.register(hk_back);
        let _ = mgr.register(hk_copy);

        Self {
            words,
            index,
            phase: Phase::WaitingForGame,
            sys: System::new(),
            last_scan: Instant::now() - Duration::from_secs(10),
            clipboard: arboard::Clipboard::new().ok(),
            _hotkeys: mgr,
            k_confirm: hk_confirm.id(),
            k_back: hk_back.id(),
            k_copy: hk_copy.id(),
            no_activate_done: false,
        }

    }

    fn save(&self) {
        let st = SavedState {
            index: self.index,
            word: self.words.get(self.index).cloned().unwrap_or_default()
        };
        if let Ok(json) = serde_json::to_string(&st) {
            let _ = std::fs::write(state_path(), json);
        }
    }

    fn copy_current(&mut self){
        if let (Some(cb), Some(word)) = (self.clipboard.as_mut(), self.words.get(self.index)) {
            let _ = cb.set_text(word.clone());
        }
    }

    fn confirm(&mut self) {
        if !matches!(self.phase, Phase::Running) || self.index >= self.words.len() {
            return;
        }
        self.index += 1;
        self.save();
        self.copy_current();
    }

    fn back(&mut self) {
        if self.index == 0 {
            return;
        }
        self.index -= 1;
        self.save();
        self.copy_current();
    }

    fn game_running(&mut self) -> bool {
        self.sys.refresh_processes(ProcessesToUpdate::All, true);
        let target = PROCESS_NAME.to_lowercase();
        self.sys
            .processes()
            .values()
            .any(|p| p.name().to_string_lossy().to_lowercase().starts_with(&target))
    }

    fn apply_no_activate(&mut self) {
        if self.no_activate_done {
            return;
        }
        unsafe {
            if let Ok(hwnd) = FindWindowW(None, w!("MasterDuel Secret Pack Helper")) {
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
                self.no_activate_done = true;
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame){
        self.apply_no_activate();

        while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            if ev.id == self.k_confirm {
                self.confirm();
            } else if ev.id == self.k_back {
                self.back()
            } else if ev.id == self.k_copy {
                self.copy_current();
            }
        }

        match self.phase {
            Phase::WaitingForGame => {
                if self.last_scan.elapsed() >= Duration::from_secs(1) {
                    self.last_scan = Instant::now();
                    if self.game_running() {
                        self.phase = Phase::Countdown(Instant::now());
                    }
                }
            }
            Phase::Countdown(start) => {
                if start.elapsed() >= LAUNCH_DELAY {
                    ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
                    self.phase = Phase::Running;
                    self.copy_current();
                }
            }
            Phase::Running => {}
        }

        let status = match &self.phase {
            Phase::WaitingForGame => format!("Waiting for '{PROCESS_NAME}' to launch"),
            Phase::Countdown(start) => {
                let left = LAUNCH_DELAY.saturating_sub(start.elapsed()).as_secs() + 1;
                format!("Game detected. Starting in {left}s...")
            }
            Phase::Running => "Running. F8 = confirm & next".to_string()
        };

        let mut do_confirm = false;
        let mut do_back = false;
        let mut do_copy = false;

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(status);
            ui.add_space(4.0);
            if self.index >= self.words.len() {
                ui.heading("All done!");
                ui.label(format!("{0} / {0}", self.words.len()));
            } else {
                ui.heading(&self.words[self.index]);
                ui.label(format!("{} / {}", self.index + 1, self.words.len()));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_sized([170.0, 40.0], egui::Button::new("Confirm -> Next (F8)"))
                    .clicked()
                {
                    do_confirm = true;
                }
                if ui
                    .add_sized([70.0, 40.0], egui::Button::new("Back (F7)"))
                    .clicked()
                {
                    do_back =true;
                }
                if ui.add_sized([70.0, 40.0], egui::Button::new("Copy (F6)"))
                    .clicked()
                {
                    do_copy = true;
                }
            });
        });

        if do_confirm {
            self.confirm();
        }
        if do_back {
            self.back();
        }
        if do_copy {
            self.copy_current();
        }

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(WINDOW_TITLE)
            .with_inner_size([360.0, 150.0])
            .with_position([20.0, 20.0])
            .with_resizable(false),
        ..Default::default()
    };

    eframe::run_native(
        WINDOW_TITLE,
        options,
        Box::new(|_cc| Ok(Box::new(App::new())))
    )
}
