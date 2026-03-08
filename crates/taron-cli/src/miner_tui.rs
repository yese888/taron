#![allow(dead_code, unused_imports)]
//! TARON Miner TUI — design fidèle à cliamp (bjarneo/cliamp)
//!
//! Rendu : frame 80 cols centrée dans le terminal.
//! Le fond du terminal reste visible autour (pas d'alternate screen).
//! Anti-flicker : overwrite ligne par ligne, pas de Clear All.

use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{disable_raw_mode, enable_raw_mode, size, Clear, ClearType},
};
use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const C_TITLE:  Color = Color::White;       // blanc — titre principal
const C_ACCENT: Color = Color::Grey;        // gris clair — valeurs
const C_PLAY:   Color = Color::White;       // blanc — ▶ Mining
const C_DIM:    Color = Color::DarkGrey;    // gris foncé — labels, séparateurs
const C_TEXT:   Color = Color::Grey;        // gris — texte secondaire
const C_BAR:    Color = Color::Grey;        // gris — barre activité
const C_ERR:    Color = Color::DarkGrey;    // pour pause

const FRAME_W: usize = 80;  // frame total width  (comme cliamp)
const PAD_H:   usize = 3;   // padding horizontal (comme lipgloss Padding(1,3))
const INNER_W: usize = FRAME_W - PAD_H * 2; // 74 — inner content width

pub struct MinerSolution {
    pub number:      u64,
    pub nonce:       u64,
    pub hash_prefix: String,
    pub reward_tar:  f64,
    pub balance_tar: f64,
    pub thread_id:   u32,
}

pub struct MinerTuiState {
    pub total_hashes:   Arc<AtomicU64>,
    pub solutions:      Arc<AtomicU64>,
    pub running:        Arc<AtomicBool>,
    pub paused:         Arc<AtomicBool>,
    pub threads:        u32,
    pub difficulty:     u32,
    pub start_time:     Instant,
    pub solution_log:   Arc<Mutex<Vec<MinerSolution>>>,
    pub wallet_address: String,
    pub testnet:        bool,
}

impl MinerTuiState {
    pub fn new(
        total_hashes: Arc<AtomicU64>, solutions: Arc<AtomicU64>,
        running: Arc<AtomicBool>, threads: u32, difficulty: u32,
        wallet_address: String, testnet: bool,
        solution_log: Arc<Mutex<Vec<MinerSolution>>>,
    ) -> Self {
        Self {
            total_hashes, solutions, running,
            paused: Arc::new(AtomicBool::new(false)),
            threads, difficulty,
            start_time: Instant::now(),
            solution_log, wallet_address, testnet,
        }
    }

    fn uptime(&self) -> String {
        let s = self.start_time.elapsed().as_secs();
        let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
        if h > 0 { format!("{:02}:{:02}:{:02}", h, m, sec) }
        else { format!("{:02}:{:02}", m, sec) }
    }

    fn hashrate(&self) -> f64 {
        let h = self.total_hashes.load(Ordering::Relaxed);
        let e = self.start_time.elapsed().as_secs_f64();
        if e > 0.0 { h as f64 / e } else { 0.0 }
    }
}

// ─── Entry point ─────────────────────────────────────────────────────────────

pub fn run_miner_tui(state: &mut MinerTuiState) -> io::Result<()> {
    let mut stdout = io::stdout();
    enable_raw_mode()?;
    // NE PAS entrer en alternate screen — on veut le fond terminal visible autour
    execute!(stdout, Hide)?;
    // Vider l'écran une seule fois au démarrage
    execute!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;

    let res = tui_loop(state, &mut stdout);

    execute!(stdout, Show)?;
    disable_raw_mode()?;
    // Laisser un curseur propre en bas du frame
    println!();
    res
}

fn tui_loop(state: &mut MinerTuiState, out: &mut io::Stdout) -> io::Result<()> {
    loop {
        draw(state, out)?;

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    match key.code {
                        // Ctrl+C ou Ctrl+Q ou Q ou Esc → quitter
                        KeyCode::Char('c') if ctrl => {
                            state.running.store(false, Ordering::Relaxed);
                            break;
                        }
                        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                            state.running.store(false, Ordering::Relaxed);
                            break;
                        }
                        KeyCode::Char('p') | KeyCode::Char('P') => {
                            let cur = state.paused.load(Ordering::Relaxed);
                            state.paused.store(!cur, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }
            }
        }

        if !state.running.load(Ordering::Relaxed) { break; }
    }
    Ok(())
}

// ─── Draw ────────────────────────────────────────────────────────────────────
//
// Approche anti-flicker : on N'efface PAS l'écran.
// On positionne le curseur au début de chaque ligne (MoveTo)
// et on écrase avec le nouveau contenu + espaces jusqu'à FRAME_W.
// À la fin, Clear(FromCursorDown) efface les lignes restantes.

fn draw(s: &MinerTuiState, out: &mut io::Stdout) -> io::Result<()> {
    let (cols, rows) = size().unwrap_or((80, 24));

    // snapshots atomics
    let hashes   = s.total_hashes.load(Ordering::Relaxed);
    let sols     = s.solutions.load(Ordering::Relaxed);
    let hashrate = s.hashrate();
    let watts    = (s.threads as f64 * 8.0).min(65.0);
    let hpw      = if watts > 0.0 { hashrate / watts } else { 0.0 };
    let paused   = s.paused.load(Ordering::Relaxed);
    let elapsed  = s.start_time.elapsed().as_secs_f64();

    // Largeur dynamique — remplit le terminal (min 80, max 200)
    let frame_w  = (cols as usize).saturating_sub(2).min(200).max(80);
    let inner_w  = frame_w.saturating_sub(PAD_H * 2);
    let pad_left = 0usize;
    let sol_rows = (rows as usize).saturating_sub(14).max(2);

    // Nombre de lignes total dans le frame
        #[allow(unused_variables)]
    let frame_lines: usize = 1          // blank top
        + 1 + 1                         // title + subtitle
        + 1                             // blank
        + 1 + 1                         // hashrate + stats
        + 1                             // blank
        + 1                             // activity bar
        + 1                             // blank
        + 1 + 1                         // HASHES line + blank
        + 1                             // separator
        + sol_rows                      // solution lines
        + 1 + 1;                        // blank + help

    // Pas de centrage vertical — contenu en haut
    let pad_top = 1usize;

    let mut row = 0u16;

    // Macro locale : écrit une ligne à la position `row`, puis incrémente row.
    // Chaque ligne est padée à FRAME_W pour écraser le contenu précédent.
    macro_rules! wline {
        ($content:expr) => {{
            queue!(out, MoveTo(pad_left as u16, pad_top as u16 + row))?;
            let s_content: String = $content;
            // On écrit le contenu puis on efface le reste de la zone frame
            let visible = strip_ansi_len(&s_content);
            let pad_right = frame_w.saturating_sub(PAD_H + visible + PAD_H);
            queue!(out,
                Print(&" ".repeat(PAD_H)),  // padding gauche
                Print(&s_content),
                Print(&" ".repeat(pad_right)), // efface l'ancien contenu
                Print(&" ".repeat(PAD_H)),  // padding droit
            )?;
            row += 1;
        }};
    }

    // ── blank top (padding vertical cliamp Padding(1,x)) ──
    wline!("".to_string());

    // ── T A R O N  M I N E R ──
    {
        let content = build_colored(|buf| {
            queue!(buf, SetForegroundColor(C_TITLE), SetAttribute(Attribute::Bold),
                Print("T A R O N  M I N E R"),
                SetAttribute(Attribute::Reset), ResetColor)
        });
        wline!(content);
    }

    // ── ⛏ testnet · address                     00:01:23  ▶ Mining ──
    {
        let net    = "testnet"; // mainnet non disponible pour l'instant
        let addr   = shorten(&s.wallet_address, 16);
        let uptime = s.uptime();
        let status = if paused { "⏸ Paused" } else { "▶ Mining" };
        let left   = format!("⛏  {}  ·  {}", net, addr);
        let right  = format!("{}  {}", uptime, status);
        let gap    = inner_w.saturating_sub(left.chars().count() + right.chars().count());
        let sc     = if paused { C_ERR } else { C_PLAY };
        let content = build_colored(|buf| {
            queue!(buf,
                SetForegroundColor(C_DIM), Print(&left),
                Print(&" ".repeat(gap)),
                SetForegroundColor(sc), SetAttribute(Attribute::Bold), Print(&right),
                SetAttribute(Attribute::Reset), ResetColor)
        });
        wline!(content);
    }

    // ── blank ──
    wline!("".to_string());

    // ── 12.50 kH/s ──
    {
        let (v, u) = fmt_hr(hashrate);
        let content = build_colored(|buf| {
            queue!(buf, SetForegroundColor(C_ACCENT), SetAttribute(Attribute::Bold),
                Print(format!("{:.2} {}", v, u)),
                SetAttribute(Attribute::Reset), ResetColor)
        });
        wline!(content);
    }

    // ── 24 threads · 3 solutions · 65 W · 120 H/W · 52°C ──
    {
        let hpw_str = if hpw >= 1000.0 {
            format!("{:.1} kH/W", hpw / 1000.0)
        } else {
            format!("{:.0} H/W", hpw)
        };
        let temp_opt = read_cpu_temp();
        let content = build_colored(|buf| {
            queue!(buf, SetForegroundColor(C_DIM),
                Print(format!("{} threads  ·  {} solutions  ·  {:.0} W  ·  {}",
                    s.threads, sols, watts, hpw_str))
            )?;
            if let Some(temp) = temp_opt {
                // Couleur selon température (style btop)
                let temp_color = if temp >= 80.0 {
                    Color::Red
                } else if temp >= 65.0 {
                    Color::DarkYellow
                } else {
                    C_DIM
                };
                queue!(buf,
                    SetForegroundColor(C_DIM), Print("  ·  "),
                    SetForegroundColor(temp_color),
                    Print(format!("{:.0}°C", temp)),
                    ResetColor
                )?;
            }
            queue!(buf, ResetColor)
        });
        wline!(content);
    }

    // ── blank ──
    wline!("".to_string());

    // ── ━━━●━━━  78% ──
    {
        let theoretical = s.threads as f64 * elapsed;
        let pct   = if theoretical > 0.0 { (hashes as f64 / theoretical).min(1.0) } else { 0.0 };
        let bw    = inner_w.saturating_sub(7);
        let fill  = (pct * bw as f64) as usize;
        let rest  = bw.saturating_sub(fill);
        let content = build_colored(|buf| {
            queue!(buf,
                SetForegroundColor(C_BAR), Print(&"━".repeat(fill)), Print("●"),
                SetForegroundColor(C_DIM), Print(&"━".repeat(rest)),
                Print(format!("  {:3.0}%", pct * 100.0)),
                ResetColor)
        });
        wline!(content);
    }

    // ── blank ──
    wline!("".to_string());

    // ── HASHES 847.3k    DIFF 16 bits ──
    {
        let hstr  = fmt_count(hashes);
        let content = build_colored(|buf| {
            queue!(buf,
                SetForegroundColor(C_TEXT), SetAttribute(Attribute::Bold), Print("HASHES "),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(C_ACCENT), Print(format!("{:<10}", hstr)),
                SetForegroundColor(C_TEXT), SetAttribute(Attribute::Bold), Print("  DIFF "),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(C_DIM), Print(format!("{} bits", s.difficulty)),
                ResetColor)
        });
        wline!(content);
    }

    // ── blank ──
    wline!("".to_string());

    // ── Solutions ─────────────────── ──
    {
        let label = "── Solutions ";
        let rest  = "─".repeat(inner_w.saturating_sub(label.chars().count()));
        let content = build_colored(|buf| {
            queue!(buf, SetForegroundColor(C_DIM), Print(label), Print(&rest), ResetColor)
        });
        wline!(content);
    }

    // ── solution lines ──
    {
        let log   = s.solution_log.lock().unwrap();
        let total = log.len();
        let mut count = 0;

        if log.is_empty() {
            let content = build_colored(|buf| {
                queue!(buf, SetForegroundColor(C_DIM),
                    Print("  No solutions yet — keep hashing!"), ResetColor)
            });
            wline!(content);
            count = 1;
        } else {
            for sol in log.iter().rev().take(sol_rows) {
                let latest  = sol.number == total as u64;
                let sym     = if latest { "▶" } else { " " };
                let txt     = format!(
                    "{} #{:<3}   nonce: {:<12}   hash: {}…   +{:.2} TAR   t{}",
                    sym, sol.number, sol.nonce,
                    sol.hash_prefix, sol.reward_tar, sol.thread_id
                );
                let trimmed = truncate(&txt, inner_w);
                let clr     = if latest { C_PLAY } else { C_DIM };
                let content = build_colored(|buf| {
                    if latest { queue!(buf, SetAttribute(Attribute::Bold))?; }
                    queue!(buf, SetForegroundColor(clr), Print(&trimmed),
                        SetAttribute(Attribute::Reset), ResetColor)
                });
                wline!(content);
                count += 1;
            }
        }

        for _ in count..sol_rows {
            wline!("".to_string());
        }
    }

    // ── blank ──
    wline!("".to_string());

    // ── [Q]Quit  [P]Pause/Resume ──
    {
        let content = build_colored(|buf| {
            hkey_buf(buf, "Ctrl+C", "Quit  ")?;
            hkey_buf(buf, "P", "Pause/Resume")
        });
        wline!(content);
    }

    // Effacer tout ce qui est en dessous (Clear::FromCursorDown suffit, pas de flicker)
    queue!(out, MoveTo(0, pad_top as u16 + row), Clear(ClearType::FromCursorDown))?;
    out.flush()
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Construit une String contenant des escape sequences ANSI via un closure.
/// Utilisé pour mesurer la longueur visible séparément.
fn build_colored<F>(f: F) -> String
where F: FnOnce(&mut Vec<u8>) -> io::Result<()>
{
    let mut buf = Vec::new();
    let _ = f(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Longueur visible d'une string (ignore les escape sequences ANSI).
fn strip_ansi_len(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut len = 0usize;
    let mut i   = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\x1b' && i + 1 < bytes.len() && bytes[i+1] == b'[' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'm' { i += 1; }
            i += 1;
        } else {
            // Count unicode chars (multi-byte)
            if bytes[i] & 0x80 == 0 {
                len += 1; i += 1;
            } else if bytes[i] & 0xE0 == 0xC0 {
                len += 1; i += 2;
            } else if bytes[i] & 0xF0 == 0xE0 {
                len += 1; i += 3;
            } else {
                len += 1; i += 4;
            }
        }
    }
    len
}

fn hkey_buf(buf: &mut Vec<u8>, key: &str, label: &str) -> io::Result<()> {
    queue!(buf,
        SetForegroundColor(C_DIM),    Print("["),
        SetForegroundColor(C_ACCENT), SetAttribute(Attribute::Bold), Print(key),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(C_DIM),    Print("]"),
        SetForegroundColor(C_DIM),    Print(label), Print("  "),
        ResetColor
    )
}

/// Lit la température CPU depuis sysfs (Linux) ou retourne None.
/// Même source que bpytop/btop : /sys/class/thermal/thermal_zoneN/temp
fn read_cpu_temp() -> Option<f64> {
    for i in 0..8 {
        let path = format!("/sys/class/thermal/thermal_zone{}/temp", i);
        if let Ok(raw) = std::fs::read_to_string(&path) {
            if let Ok(millideg) = raw.trim().parse::<i64>() {
                let celsius = millideg as f64 / 1000.0;
                if celsius > 0.0 && celsius < 120.0 {
                    return Some(celsius);
                }
            }
        }
    }
    None
}

fn fmt_hr(h: f64) -> (f64, &'static str) {
    if h >= 1_000_000.0 { (h / 1_000_000.0, "MH/s") }
    else if h >= 1_000.0 { (h / 1_000.0,     "kH/s") }
    else                  { (h,               " H/s") }
}

fn fmt_count(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1_000_000.0) }
    else if n >= 1_000 { format!("{:.1}k", n as f64 / 1_000.0) }
    else               { format!("{}", n) }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else { s.to_string() }
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max { chars[..max].iter().collect() }
    else { s.to_string() }
}
