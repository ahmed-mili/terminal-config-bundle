// Port Rust de statusline.ps1 -- invoque par claude.exe sur chaque evenement de
// conversation ET toutes les `statusLine.refreshInterval` secondes (settings.json).
//
// Cadence maximale NATIVE : 1 Hz. Verifie dans le schema settings de claude.exe
// 2.1.211 : `refreshInterval: number().min(1).optional().catch(undefined)` -- une
// valeur < 1 (ex. 0.1) est REJETEE par le schema et il ne reste AUCUN refresh
// periodique ; cote UI le timer fait de toute facon Math.max(1, j)*1000. Ne jamais
// remettre 0.1 : c'etait l'objectif historique du binaire, invalide depuis.
//
// L'enjeu du binaire natif : PowerShell met ~420 ms par spawn, donc claude.exe
// (abort du subprocess a ~100 ms au tick suivant) le tuait avant sa sortie. Un
// binaire natif demarre en ~10 ms = marge confortable meme a 1 Hz.

use std::fs;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Datelike, Local, TimeZone, Utc, Weekday};
use serde::Deserialize;
use serde_json::Value;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const RESET: &str = "\x1b[0m";

fn rgb(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{};{};{}m", r, g, b)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn bg(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[48;2;{};{};{}m", r, g, b)
}

fn file_age_secs(path: &Path) -> Option<f64> {
    let m = fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?;
    Some(
        SystemTime::now()
            .duration_since(mtime)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0),
    )
}

fn file_mtime_utc(path: &Path) -> Option<DateTime<Utc>> {
    let m = fs::metadata(path).ok()?;
    let mtime = m.modified().ok()?;
    let d = mtime.duration_since(UNIX_EPOCH).ok()?;
    Utc.timestamp_opt(d.as_secs() as i64, d.subsec_nanos()).single()
}

fn touch(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // ECRIRE des octets, pas juste File::create : re-creer (truncate) un
    // fichier VIDE n'ecrit rien et NTFS ne bump alors PAS LastWriteTime.
    // Consequence historique : tous les cooldowns bases sur file_age_secs
    // (git fetch 30 s, usage refresh 55 s, ollama 15 s) etaient morts -- spawn
    // a CHAQUE tick, rafales concurrentes sur /api/oauth/usage -> 429
    // chronique auto-inflige (constate 2026-07-17, marqueur fige au 25 juin).
    // Le contenu (now_ms) est purement informatif ; seul le write compte.
    let _ = fs::write(path, now_ms().to_string().as_bytes());
}

fn run_git(dir: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok().map(|s| s.trim_end().to_string())
}

// =================== USAGE / EFFORT / FORMATTING HELPERS ===================

fn get_usage_color(pct: f64, stale: bool) -> String {
    // Palette inspiree de la barre de contexte de claude.ai (couleurs relevees au
    // pixel pres), bleu eclairci pour mieux ressortir sur fond sombre. Trois paliers :
    //   0-69 %  -> bleu   #50A0F0 rgb(80,160,240)  (claude.ai #2A78D6 eclairci)
    //   70-89 % -> jaune  #FAB219 rgb(250,178,25)
    //   >= 90 % -> rouge  #D03B3B rgb(208,59,59)
    // En mode stale (cache d'usage perime), memes teintes mais desaturees/ternies.
    if stale {
        if pct < 70.0 { return rgb(130, 165, 205); }
        if pct < 90.0 { return rgb(200, 180, 120); }
        return rgb(190, 125, 125);
    }
    if pct < 70.0 { return rgb(80, 160, 240); }
    if pct < 90.0 { return rgb(250, 178, 25); }
    rgb(208, 59, 59)
}

// Couleur du contexte de fenetre (ligne 1, ex. "136k/1.0M tok"). Distincte de la
// palette d'usage (bleu/jaune/rouge facon claude.ai) : le contexte n'est pas un
// quota de session, donc bas niveau = vert (de la place libre) plutot que bleu.
//   0-69 %  -> vert  #50FA7B rgb(80,250,123)
//   70-89 % -> jaune #FAB219 rgb(250,178,25)
//   >= 90 % -> rouge #D03B3B rgb(208,59,59)
fn get_context_color(pct: f64) -> String {
    if pct < 70.0 { return rgb(80, 250, 123); }
    if pct < 90.0 { return rgb(250, 178, 25); }
    rgb(208, 59, 59)
}

fn format_tokens(n: i64) -> String {
    if n < 1000 { return n.to_string(); }
    if n < 1_000_000 {
        let v = (n as f64 / 1000.0).round() as i64;
        return format!("{}k", v);
    }
    let v = (n as f64 / 1_000_000.0 * 10.0).round() / 10.0;
    // Force '.' decimal (Rust default, donc OK).
    format!("{:.1}M", v)
}

fn format_bar(pct: f64, col: &str, width: usize) -> String {
    let mut filled = (pct / 100.0 * width as f64).round() as i64;
    if filled > width as i64 { filled = width as i64; }
    if filled < 0 { filled = 0; }
    let filled = filled as usize;
    let empty = width - filled;
    // U+2501 est un glyphe box-drawing jointif entre cellules, contrairement a
    // U+25AC (rectangle geometrique) qui conserve des marges laterales visibles.
    // Piste (track) calquee sur claude.ai : gris sombre #424240 rgb(66,66,64).
    let rail = rgb(66, 66, 64);
    format!(
        "{}{}{}{}{}",
        col,
        "\u{2501}".repeat(filled),
        rail,
        "\u{2501}".repeat(empty),
        RESET
    )
}

// Cadence interne du picker /effort de claude.exe : M=112ms (~9 Hz). Le rendu
// des effort levels cote statusline est desormais statique (cf.
// get_effort_display), donc picker_tick ne sert plus qu'au log
// d'instrumentation (statusline-tick-log.txt) comme identifiant deterministe
// d'un tick -- utile pour correler des ticks rapproches sans coller un
// timestamp ms qui change a chaque invocation.
const PICKER_ZC: i64 = 16;
const PICKER_H: i64 = 100;
const PICKER_M: i64 = ((PICKER_H + PICKER_ZC - 1) / PICKER_ZC) * PICKER_ZC; // = 112

fn picker_tick(now_ms: i64) -> i64 {
    let k = (now_ms / PICKER_M) * PICKER_M;
    k / PICKER_H
}

fn get_effort_display(level: Option<&str>) -> String {
    let Some(level) = level else { return String::new(); };
    if level.is_empty() { return String::new(); }
    let bold = "\x1b[1m";
    let rst = "\x1b[0m";
    let label = level;

    // Rendu statique. Les couleurs sont des codes ANSI de slots de palette du
    // terminal, PAS des RGB figes : on emet exactement le meme SGR que
    // claude.exe pour chaque cible -> rendu strictement identique quel que soit
    // le theme du terminal (ici Catppuccin Mocha : slot 9 = #F38BA8, slot 13 =
    // #F5C2E7) et le reglage intenseTextStyle.
    //   low/medium/high -> ANSI bright 93/92/94 + bold (design statusline, pas
    //                      de cible externe a matcher).
    //   xhigh -> ANSI 95 (magentaBright), SANS gras = couleur EXACTE de
    //            l'indicateur "⏵⏵ accept edits on". Verifie dans le binaire :
    //            acceptEdits -> color:"autoAccept" ; dark-ansi mappe autoAccept
    //            -> ansi:magentaBright -> chalk.magentaBright -> \x1b[95m. C'est
    //            aussi la couleur de base des lettres de l'ancien xhigh anime
    //            (le halo balayant #D0B4FF n'est PAS voulu) -- cf. git 5c3b0c3.
    //   max   -> ANSI 91 (bright red), SANS gras = couleur EXACTE de
    //            l'indicateur "⏵⏵ bypass permissions on". Verifie :
    //            bypassPermissions -> color:"error" ; dark-ansi mappe error ->
    //            ansi:redBright -> chalk.redBright -> \x1b[91m.
    // Les indicateurs de claude.exe sont rendus color-only -- createElement(
    // Text, {color}, glyph, " ", label), AUCUN bold ni dim. On reproduit donc
    // uniquement le code couleur (sans \x1b[1m) : match au pixel pres, robuste
    // au theme, a la police (JetBrainsMono Nerd Font) et a intenseTextStyle.
    match level {
        "low" => format!("\x1b[93m{}{}{}", bold, label, rst),
        "medium" => format!("\x1b[92m{}{}{}", bold, label, rst),
        "high" => format!("\x1b[94m{}{}{}", bold, label, rst),
        "xhigh" => format!("\x1b[95m{}{}", label, rst),
        "max" => format!("\x1b[91m{}{}", label, rst),
        _ => String::new(),
    }
}

fn format_reset(reset_at: &Value, reference: Option<DateTime<Utc>>) -> Option<String> {
    let s = reset_at.as_str()?;
    let reset_utc = DateTime::parse_from_rfc3339(s).ok()?.with_timezone(&Utc);
    let now = reference.unwrap_or_else(Utc::now);
    let delta = reset_utc.signed_duration_since(now);
    if delta.num_seconds() <= 0 {
        return Some("now".to_string());
    }
    let total_minutes = delta.num_seconds() as f64 / 60.0;
    if total_minutes < 60.0 {
        return Some(format!("{}m", total_minutes.floor() as i64));
    }
    let total_hours = delta.num_seconds() as f64 / 3600.0;
    if total_hours < 24.0 {
        let h = total_hours.floor() as i64;
        let m = (delta.num_seconds() - h * 3600) / 60;
        return Some(format!("{}h{:02}m", h, m));
    }
    let local = reset_utc.with_timezone(&Local);
    let day_abbr = match local.weekday() {
        Weekday::Sun => "dim",
        Weekday::Mon => "lun",
        Weekday::Tue => "mar",
        Weekday::Wed => "mer",
        Weekday::Thu => "jeu",
        Weekday::Fri => "ven",
        Weekday::Sat => "sam",
    };
    Some(format!("{}. {}", day_abbr, local.format("%H:%M")))
}

// Age compact pour le marqueur "(perime <age>)" d'une fenetre expiree : secondes
// depuis le reset rate (toujours >= 0 dans ce contexte). Bornes lisibles a coup
// d'oeil : s -> m -> h -> j.
fn fmt_age(secs: i64) -> String {
    let s = secs.max(0);
    if s < 60 { return format!("{}s", s); }
    let m = s / 60;
    if m < 60 { return format!("{}m", m); }
    let h = m / 60;
    if h < 24 { return format!("{}h", h); }
    format!("{}j", h / 24)
}

// =================== GIT ===================

#[derive(Default)]
struct GitInfo {
    branch: Option<String>,
    sha: Option<String>,
    ahead: i32,
    behind: i32,
    dirty: i32,
    fetch_stale: bool,
}

fn find_git_root(start: &str) -> Option<PathBuf> {
    let mut probe = PathBuf::from(start);
    loop {
        if probe.join(".git").exists() {
            return Some(probe);
        }
        if !probe.pop() {
            return None;
        }
    }
}

fn compute_git(dir: &str) -> GitInfo {
    let mut info = GitInfo::default();
    let Some(root) = find_git_root(dir) else { return info; };

    // Single git call: status --porcelain=v2 --branch returns branch, oid, ab, AND dirty.
    // Saves ~30-50ms vs an extra `git rev-parse --abbrev-ref HEAD` spawn on Windows,
    // which used to push total time over CC's ~100ms abort threshold in git repos.
    let Some(out) = run_git(dir, &["status", "--porcelain=v2", "--branch"]) else {
        return info;
    };

    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            let head = rest.trim();
            if !head.is_empty() {
                // Detached HEAD -> "(detached)" ; aligne sur ce que `rev-parse --abbrev-ref HEAD` renvoyait ("HEAD")
                info.branch = Some(if head == "(detached)" { "HEAD".to_string() } else { head.to_string() });
            }
        } else if let Some(rest) = line.strip_prefix("# branch.oid ") {
            let oid = rest.trim();
            if oid.len() >= 7 && oid != "(initial)" {
                info.sha = Some(oid[..7].to_string());
            }
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // format: +N -M
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() == 2 {
                if let Some(a) = parts[0].strip_prefix('+') {
                    info.ahead = a.parse().unwrap_or(0);
                }
                if let Some(b) = parts[1].strip_prefix('-') {
                    info.behind = b.parse().unwrap_or(0);
                }
            }
        } else if let Some(first) = line.chars().next() {
            // Premier char : 1=change, 2=renomme, ?=untracked, u=unmerged
            if matches!(first, '1' | '2' | '?' | 'u') && line.chars().nth(1) == Some(' ') {
                info.dirty += 1;
            }
        }
    }

    if info.branch.is_none() {
        return info;
    }

    // Background fetch cooldown 30s
    let marker = root.join(".git").join("statusline-last-fetch");
    let needs_fetch = match file_age_secs(&marker) {
        Some(age) => age >= 30.0,
        None => true,
    };
    if needs_fetch {
        // Touch d'abord pour empecher d'autres ticks de re-spawn pendant que celui-ci demarre.
        touch(&marker);
        // Le spawn lui-meme coute ~300 ms sur Windows quand Defender realtime est actif :
        // chaque creation de process git est scannee. DETACHED_PROCESS ne sauve pas ce cout,
        // il rend juste le process enfant detache une fois cree. On detache donc aussi LA CREATION
        // elle-meme dans un thread daemon, pour que la main thread retourne immediatement.
        let dir_owned = dir.to_string();
        std::thread::spawn(move || {
            let mut cmd = Command::new("git");
            cmd.args(["-C", &dir_owned, "fetch", "--quiet"]);
            cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            #[cfg(windows)]
            cmd.creation_flags(CREATE_NO_WINDOW | 0x00000008 /* DETACHED_PROCESS */);
            let _ = cmd.spawn();
        });
    }

    // FETCH_HEAD staleness check
    let fetch_head = root.join(".git").join("FETCH_HEAD");
    if let Some(age) = file_age_secs(&fetch_head) {
        if age > 150.0 {
            info.fetch_stale = true;
        }
    }

    info
}

// =================== USAGE / AUTH ===================

#[derive(Deserialize)]
struct CredsRoot {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<CredsOauth>,
}
#[derive(Deserialize)]
struct CredsOauth {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}

fn read_credentials(claude_dir: &Path) -> Option<CredsRoot> {
    let path = claude_dir.join(".credentials.json");
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

// Empreinte non reversible du token OAuth courant. Sert a detecter un changement
// de compte (/login reecrit .credentials.json avec un nouveau accessToken) sans
// stocker le token en clair ni dependre du mtime du fichier (peu fiable a cause du
// cache de metadonnees NTFS). Un refresh de token (meme compte) change aussi
// l'empreinte -> declenche juste un refresh API supplementaire, inoffensif.
fn token_fingerprint(token: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn current_token_fingerprint(claude_dir: &Path) -> Option<String> {
    read_credentials(claude_dir)
        .and_then(|c| c.claude_ai_oauth)
        .and_then(|o| o.access_token)
        .map(|t| token_fingerprint(&t))
}

// Cache 1h de la version de claude.exe pour le User-Agent. Si l'API Anthropic
// commence un jour a verifier strictement le UA, hard-coder "2.0.32" devient
// une bombe a retardement. On extrait la version via `claude --version`
// (sortie : "2.1.148 (Claude Code)\n"), cachee dans claude-version.txt.
// Fallback "2.0.32" si claude introuvable -- valeur historique connue pour
// fonctionner avec l'endpoint /api/oauth/usage.
fn detect_claude_version(claude_dir: &Path) -> String {
    let cache_path = claude_dir.join("claude-version.txt");
    if let Some(age) = file_age_secs(&cache_path) {
        if age < 3600.0 {
            if let Ok(s) = fs::read_to_string(&cache_path) {
                let v = s.trim().to_string();
                if !v.is_empty() {
                    return v;
                }
            }
        }
    }
    let mut cmd = Command::new("claude");
    cmd.arg("--version");
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let version = cmd
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .filter(|s| !s.is_empty() && s.chars().next().map_or(false, |c| c.is_ascii_digit()))
        .unwrap_or_else(|| "2.0.32".to_string());
    let _ = fs::write(&cache_path, &version);
    version
}

#[derive(Debug, Clone, Copy)]
enum UsageSource {
    Stdin,              // rate_limits.* directement dans le stdin JSON (Pro/Max
                        // apres 1er API call). Source la plus fiable -- pas
                        // d'appel HTTP, pas de token, pas de cache.
    StdinCacheMerged,   // stdin dont au moins une fenetre expiree a ete
                        // remplacee par la version plus fraiche du cache API
                        // (session idle apres un reset de fenetre).
    CacheFresh,
    ApiSuccess,
    ApiSuccessRetry,    // succes au 2e essai apres retry 500ms
    ApiTimeoutOrIo,     // echec apres 1 essai (+1 retry) -- reseau down ou DNS
    Api429,
    Api401,             // token expire -- claude refresh au prochain api call
    ApiBadStatus,       // autre code 4xx/5xx
    Cooldown,
    StaleCacheFallback,
    NoCredsOrNoCache,
}

struct UsageResult {
    json: Option<Value>,
    stale: bool,
    reference: Option<DateTime<Utc>>,
    source: UsageSource,
    api_ms: Option<u128>,
    api_status: Option<u16>,
    api_attempts: u8,
}

// Resultat d'un essai HTTP unique. Distingue les classes d'erreurs pour decider
// quoi faire :
//   - Io/Timeout -> RETRY (peut etre transitoire : DNS hiccup, packet loss)
//   - Status(code) -> PAS de retry (server-side decision : 429, 401, 5xx ne se
//     resolvent pas en 500ms)
//   - BodyParse -> PAS de retry (le serveur a renvoye 200 OK mais avec un corps
//     non-JSON : probablement une page d'erreur HTML, structurelle)
enum FetchOutcome {
    Ok(Value),
    Io,
    Status(u16),
    BodyParse,
}

fn fetch_usage_once(agent: &ureq::Agent, token: &str, user_agent: &str) -> FetchOutcome {
    match agent
        .get("https://api.anthropic.com/api/oauth/usage")
        .set("Authorization", &format!("Bearer {}", token))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("User-Agent", user_agent)
        .set("Accept", "application/json, text/plain, */*")
        .set("Content-Type", "application/json")
        .call()
    {
        Ok(resp) => match resp.into_string() {
            Ok(body) => match serde_json::from_str::<Value>(&body) {
                Ok(v) => FetchOutcome::Ok(v),
                Err(_) => FetchOutcome::BodyParse,
            },
            Err(_) => FetchOutcome::Io,
        },
        Err(ureq::Error::Status(code, _)) => FetchOutcome::Status(code),
        Err(ureq::Error::Transport(_)) => FetchOutcome::Io,
    }
}

// Convertit le bloc rate_limits du stdin Claude Code en JSON format usage-cache
// (utilization + resets_at ISO). Documente :
//   https://code.claude.com/docs/en/statusline#full-json-schema
//   rate_limits = { five_hour: { used_percentage, resets_at }, seven_day: {...} }
//   resets_at est un Unix epoch en SECONDES dans le stdin (vs ISO 8601 dans
//   l'API endpoint /api/oauth/usage). On normalise vers ISO 8601 pour que
//   `format_reset` n'ait qu'une seule branche.
// Absent pour : (a) sessions early avant 1er API call, (b) users API direct
// (non Pro/Max). Dans ces cas le caller fallback sur l'API HTTP.
fn build_usage_from_stdin_rate_limits(data: &Value) -> Option<Value> {
    let rl = data.get("rate_limits")?;
    let mut result = serde_json::Map::new();

    for key in ["five_hour", "seven_day"] {
        if let Some(window) = rl.get(key) {
            let pct = window.get("used_percentage").and_then(|v| v.as_f64());
            if let Some(p) = pct {
                let mut obj = serde_json::Map::new();
                obj.insert("utilization".to_string(), Value::from(p));
                if let Some(epoch) = window.get("resets_at").and_then(|v| v.as_i64()) {
                    if let Some(dt) = Utc.timestamp_opt(epoch, 0).single() {
                        obj.insert("resets_at".to_string(), Value::from(dt.to_rfc3339()));
                    }
                }
                result.insert(key.to_string(), Value::Object(obj));
            }
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(Value::Object(result))
    }
}

// Fenetre "vivante" = resets_at present, parseable, strictement futur. Une
// fenetre expiree decrit la fenetre PRECEDENTE (le stdin de claude.exe fige
// tant qu'aucun appel API principal n'est refait -- source du "perime" qui
// collait a l'ecran sur session idle).
fn window_alive(w: Option<&Value>, now: DateTime<Utc>) -> bool {
    w.and_then(|x| x.get("resets_at"))
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc) > now)
        .unwrap_or(false)
}

// Merge stdin <-> cache API par fenetre : la donnee VIVANTE gagne.
//   - stdin vivant -> stdin (autoritaire, cas nominal, zero cout).
//   - stdin expire/absent + cache vivant -> cache (rempli ~1/min par
//     `--refresh-usage`, seule source qui bouge sur une session idle).
//   - stdin expire + cache interroge APRES le reset (fetched_at > resets_at)
//     sans fenetre vivante -> la fenetre est reellement close, aucune session
//     n'a redemarre : on affiche 0 % (sans resets_at) au lieu d'un "perime"
//     qui ne se dissipera jamais.
//   - sinon -> stdin tel quel (l'affichage neutralise en "perime", le refresh
//     API suivant corrigera en <= ~55 s).
// seven_day_opus (jamais dans le stdin) et fetched_at (date du dernier fetch
// API reel, ecrite par run_usage_refresh) sont recopies du cache pour survivre
// aux ecritures stdin du cache. Retourne (merged, au_moins_une_fenetre_du_cache).
fn merge_usage_windows(stdin_data: Value, cached: Option<&Value>, now: DateTime<Utc>) -> (Value, bool) {
    let mut merged = stdin_data;
    let mut from_cache = false;
    if let Some(cache) = cached {
        for key in ["five_hour", "seven_day"] {
            if window_alive(merged.get(key), now) {
                continue; // stdin vivant -> autoritaire
            }
            let cache_w = cache.get(key);
            if window_alive(cache_w, now) {
                let replacement = cache_w.cloned().unwrap_or(Value::Null);
                if let Some(obj) = merged.as_object_mut() {
                    obj.insert(key.to_string(), replacement);
                    from_cache = true;
                }
                continue;
            }
            // stdin expire + API consultee APRES ce reset sans renvoyer de
            // fenetre vivante -> la fenetre est reellement close : 0 %.
            let stdin_reset = merged
                .get(key)
                .and_then(|w| w.get("resets_at"))
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc));
            let fetched = cache
                .get("fetched_at")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.with_timezone(&Utc));
            if let (Some(reset), Some(fetched)) = (stdin_reset, fetched) {
                if fetched > reset {
                    let mut zero = serde_json::Map::new();
                    zero.insert("utilization".to_string(), Value::from(0.0));
                    if let Some(obj) = merged.as_object_mut() {
                        obj.insert(key.to_string(), Value::Object(zero));
                        from_cache = true;
                    }
                }
            }
        }
        if let Some(obj) = merged.as_object_mut() {
            if let Some(sdo) = cache.get("seven_day_opus") {
                obj.insert("seven_day_opus".to_string(), sdo.clone());
            }
            if let Some(f) = cache.get("fetched_at") {
                obj.insert("fetched_at".to_string(), f.clone());
            }
        }
    }
    (merged, from_cache)
}

fn read_usage(
    claude_dir: &Path,
    stdin_rate_limits: Option<Value>,
    stdin_version: Option<&str>,
) -> UsageResult {
    let cache_path = claude_dir.join("usage-cache.json");
    let ratelimit_path = claude_dir.join("usage-ratelimit.txt");

    // STDIN PATH (toujours prefere : zero overhead, zero token, zero
    // dependance reseau). On enrichit avec seven_day_opus depuis le cache si
    // disponible (rate_limits du stdin ne contient pas opus -- limitation
    // documentee Anthropic, opus_usage change peu sur une window 7j donc
    // staleness moderee acceptable).
    if let Some(stdin_data) = stdin_rate_limits {
        // Rafraichissement API en arriere-plan. five_hour/seven_day du stdin sont
        // frais TANT QUE la session parle a l'API ; sur session idle ils figent
        // (fenetre expiree -> "perime"). seven_day_opus n'est de toute facon PAS
        // expose sur stdin (cf. doc officielle code.claude.com/docs/en/statusline).
        // Le cache API (usage-cache.json) est donc la seule source vivante en
        // idle. On (re)declenche son refresh quand :
        //   - notre dernier refresh date de > 55 s  -> actualisation ~1/min ;
        //   - OU le token a change (signal d'un /login : changement de compte).
        //     Dans ce cas le cache appartient a l'ANCIEN compte : on ne merge
        //     RIEN depuis le cache tant que le refresh n'a pas repopule le cache
        //     (et reecrit usage-account.txt) avec le nouveau compte.
        let refresh_marker = claude_dir.join("usage-refresh-last");
        let account_path = claude_dir.join("usage-account.txt");
        let current_fp = current_token_fingerprint(claude_dir);
        let stored_fp = fs::read_to_string(&account_path).ok().map(|s| s.trim().to_string());
        let account_changed = match (&current_fp, &stored_fp) {
            (Some(c), Some(s)) => c != s,
            (Some(_), None) => true, // jamais enregistre (bootstrap / 1er tick)
            (None, _) => false,      // pas de credentials -> rien a detecter
        };
        let stale_opus = file_age_secs(&refresh_marker).map_or(true, |a| a >= 55.0);
        if account_changed || stale_opus {
            touch(&refresh_marker);
            spawn_usage_refresh(claude_dir);
        }

        let cached_json: Option<Value> = if account_changed {
            None
        } else {
            fs::read_to_string(&cache_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        };
        let (merged, from_cache) = merge_usage_windows(stdin_data, cached_json.as_ref(), Utc::now());

        // Update cache avec le meilleur etat connu (fenetres vivantes preservees,
        // fetched_at conserve) pour le futur fallback API et les autres sessions.
        if let Ok(body) = serde_json::to_string(&merged) {
            let _ = fs::write(&cache_path, body.as_bytes());
        }
        // Si on avait un ratelimit arme avant, le retirer maintenant qu'on a
        // une donnee valide (le 429 etait peut-etre transitoire).
        let _ = fs::remove_file(&ratelimit_path);

        return UsageResult {
            json: Some(merged),
            stale: false,
            reference: None,
            source: if from_cache { UsageSource::StdinCacheMerged } else { UsageSource::Stdin },
            api_ms: None,
            api_status: None,
            api_attempts: 0,
        };
    }

    let mut usage: Option<Value> = None;
    let mut source = UsageSource::NoCredsOrNoCache;
    let mut api_ms: Option<u128> = None;
    let mut api_status: Option<u16> = None;
    let mut api_attempts: u8 = 0;

    // Cache 60s
    if let Some(age) = file_age_secs(&cache_path) {
        if age < 60.0 {
            if let Ok(raw) = fs::read_to_string(&cache_path) {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    usage = Some(v);
                    source = UsageSource::CacheFresh;
                }
            }
        }
    }

    // Cooldown 5 min apres 429
    let mut in_cooldown = false;
    if usage.is_none() {
        if let Ok(raw) = fs::read_to_string(&ratelimit_path) {
            if let Ok(ts) = DateTime::parse_from_rfc3339(raw.trim()) {
                let elapsed = Utc::now().signed_duration_since(ts.with_timezone(&Utc));
                if elapsed.num_seconds() < 300 {
                    in_cooldown = true;
                    source = UsageSource::Cooldown;
                }
            }
        }
    }

    if usage.is_none() && !in_cooldown {
        if let Some(creds) = read_credentials(claude_dir) {
            if let Some(token) = creds.claude_ai_oauth.as_ref().and_then(|o| o.access_token.clone())
            {
                // User-Agent : version depuis stdin (gratuit), sinon spawn
                // `claude --version` cache 1h, sinon fallback hardcode "2.0.32".
                let version = stdin_version
                    .map(String::from)
                    .unwrap_or_else(|| detect_claude_version(claude_dir));
                let user_agent = format!("claude-code/{}", version);
                let agent = ureq::AgentBuilder::new()
                    .timeout(Duration::from_secs(4))
                    .build();
                let t_api = Instant::now();
                api_attempts = 1;
                let mut outcome = fetch_usage_once(&agent, &token, &user_agent);

                // Retry une seule fois apres 500ms si erreur IO/timeout. Pas de
                // retry sur 4xx/5xx (le serveur a deja decide), pas sur BodyParse
                // (changement structurel cote serveur).
                if matches!(outcome, FetchOutcome::Io) {
                    std::thread::sleep(Duration::from_millis(500));
                    api_attempts = 2;
                    outcome = fetch_usage_once(&agent, &token, &user_agent);
                }
                api_ms = Some(t_api.elapsed().as_millis());

                match outcome {
                    FetchOutcome::Ok(v) => {
                        if let Ok(body) = serde_json::to_string(&v) {
                            let _ = fs::write(&cache_path, body.as_bytes());
                        }
                        let _ = fs::remove_file(&ratelimit_path);
                        usage = Some(v);
                        source = if api_attempts == 2 {
                            UsageSource::ApiSuccessRetry
                        } else {
                            UsageSource::ApiSuccess
                        };
                        api_status = Some(200);
                    }
                    FetchOutcome::Status(429) => {
                        let _ = fs::write(&ratelimit_path, Utc::now().to_rfc3339().as_bytes());
                        source = UsageSource::Api429;
                        api_status = Some(429);
                    }
                    FetchOutcome::Status(401) => {
                        // Token expire -- claude.exe le refresh au prochain api
                        // call principal. Pas la peine d'armer le cooldown 429.
                        source = UsageSource::Api401;
                        api_status = Some(401);
                    }
                    FetchOutcome::Status(code) => {
                        source = UsageSource::ApiBadStatus;
                        api_status = Some(code);
                    }
                    FetchOutcome::Io | FetchOutcome::BodyParse => {
                        source = UsageSource::ApiTimeoutOrIo;
                    }
                }
            }
        }
    }

    // Fallback stale : reutiliser le cache meme vieux
    let mut stale = false;
    let mut reference: Option<DateTime<Utc>> = None;
    if usage.is_none() && cache_path.exists() {
        if let Ok(raw) = fs::read_to_string(&cache_path) {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                usage = Some(v);
                stale = true;
                reference = file_mtime_utc(&cache_path);
                source = UsageSource::StaleCacheFallback;
            }
        }
    }

    UsageResult {
        json: usage,
        stale,
        reference,
        source,
        api_ms,
        api_status,
        api_attempts,
    }
}

// Spawn detache d'une re-invocation de soi-meme (`statusline.exe --refresh-usage`)
// qui rafraichit usage-cache.json via l'API. Meme pattern detache que le git fetch
// / spawn_context_resolver : zero blocage du chemin chaud 10 Hz. Le cooldown (touch
// du marqueur usage-refresh-last) est gere par l'appelant (read_usage), pas ici.
fn spawn_usage_refresh(claude_dir: &Path) {
    let _ = claude_dir; // chemin transmis via USERPROFILE au process enfant
    let Ok(exe) = std::env::current_exe() else { return; };
    let mut cmd = Command::new(exe);
    cmd.arg("--refresh-usage");
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | 0x00000008 /* DETACHED_PROCESS */);
    let _ = cmd.spawn();
}

// Mode detache : interroge /api/oauth/usage et ecrit usage-cache.json. Sert a
// rafraichir seven_day_opus (absent du stdin) ~1/min et apres un /login. Best-effort,
// silencieux. Respecte le cooldown 429 (usage-ratelimit.txt) pour ne pas marteler.
fn run_usage_refresh(claude_dir: &Path) {
    let cache_path = claude_dir.join("usage-cache.json");
    let ratelimit_path = claude_dir.join("usage-ratelimit.txt");

    // Cooldown 5 min apres un 429.
    if let Ok(raw) = fs::read_to_string(&ratelimit_path) {
        if let Ok(ts) = DateTime::parse_from_rfc3339(raw.trim()) {
            let elapsed = Utc::now().signed_duration_since(ts.with_timezone(&Utc));
            if elapsed.num_seconds() < 300 {
                return;
            }
        }
    }

    let Some(creds) = read_credentials(claude_dir) else { return; };
    let Some(token) = creds.claude_ai_oauth.and_then(|o| o.access_token) else { return; };
    let version = detect_claude_version(claude_dir);
    let user_agent = format!("claude-code/{}", version);
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(4))
        .build();
    let mut outcome = fetch_usage_once(&agent, &token, &user_agent);
    if matches!(outcome, FetchOutcome::Io) {
        std::thread::sleep(Duration::from_millis(500));
        outcome = fetch_usage_once(&agent, &token, &user_agent);
    }
    match outcome {
        FetchOutcome::Ok(mut v) => {
            // L'API renvoie les 3 fenetres (dont seven_day_opus). On ecrit tout,
            // date par fetched_at : le merge stdin<->cache s'en sert pour savoir
            // si l'API a ete consultee APRES l'expiration d'une fenetre stdin
            // (fenetre reellement close -> 0 %, au lieu d'un "perime" eternel).
            // Les ticks stdin preservent ce champ tel quel dans leurs ecritures.
            if let Some(obj) = v.as_object_mut() {
                obj.insert("fetched_at".to_string(), Value::from(Utc::now().to_rfc3339()));
            }
            if let Ok(body) = serde_json::to_string(&v) {
                let _ = fs::write(&cache_path, body.as_bytes());
            }
            let _ = fs::remove_file(&ratelimit_path);
            // Enregistre l'empreinte du compte pour lequel ce cache est valide :
            // le prochain tick stdin la comparera au token courant pour detecter un
            // changement de compte (/login).
            let _ = fs::write(claude_dir.join("usage-account.txt"), token_fingerprint(&token));
        }
        FetchOutcome::Status(429) => {
            let _ = fs::write(&ratelimit_path, Utc::now().to_rfc3339().as_bytes());
        }
        _ => {}
    }
}

// ================= PRE-SHAPING ARABE POUR LE TERMINAL HOTE =================
// Le pre-shaping (formes de presentation U+FExx) supplee un renderer qui ne
// fait pas le shaping contextuel : Windows Terminal, cf. microsoft/terminal#538.
// Il a un cout -- chaque forme occupe une cellule monospace pleine, donc le mot
// sort disloque (mim final detache, blancs parasites) meme quand les liaisons
// sont justes. Un terminal qui shape ET reordonne lui-meme n'en a pas besoin et
// rend nettement mieux le texte BRUT : c'est le cas de Warp, verifie a l'ecran
// le 2026-08-27 contre son propre rendu natif du meme chemin.
// Applique au chemin AFFICHE uniquement -- compute_git() recoit le chemin brut.

/// Traitement a appliquer au run arabe avant emission.
#[derive(Clone, Copy, PartialEq)]
enum ArabicMode {
    /// Aucun : le terminal shape et reordonne lui-meme (Warp).
    Raw,
    /// Formes de presentation en ordre logique, le renderer fait le BiDi (WT).
    Logical,
    /// Formes de presentation deja inversees, pour un renderer sans BiDi.
    Visual,
}

impl ArabicMode {
    fn as_str(self) -> &'static str {
        match self {
            ArabicMode::Raw => "raw",
            ArabicMode::Logical => "logical",
            ArabicMode::Visual => "visual",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "raw" => Some(ArabicMode::Raw),
            "logical" => Some(ArabicMode::Logical),
            "visual" => Some(ArabicMode::Visual),
            _ => None,
        }
    }
}

/// Override par fichier (~/.claude/statusline-bidi.txt) puis par env
/// (CLAUDE_STATUSLINE_BIDI), sinon detection du terminal. Le fichier permet de
/// basculer a chaud : l'env d'un claude.exe deja lance n'est pas modifiable, et
/// comparer deux rendus a l'ecran demande de changer de mode sans le relancer.
fn arabic_mode() -> ArabicMode {
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let p = std::path::Path::new(&home).join(".claude/statusline-bidi.txt");
        if let Some(m) = fs::read_to_string(&p).ok().and_then(|s| ArabicMode::parse(&s)) {
            return m;
        }
    }
    if let Some(m) = std::env::var("CLAUDE_STATUSLINE_BIDI")
        .ok()
        .and_then(|s| ArabicMode::parse(&s))
    {
        return m;
    }
    // Pas de detection de terminal : Warp comme Windows Terminal rendent le
    // pre-shape logique a l'identique de leur propre rendu natif du chemin
    // (mesure a l'ecran le 2026-08-27). Raw et Visual restent joignables par
    // override, pour re-mesurer si un autre terminal se comporte autrement.
    ArabicMode::Logical
}

/// (isolated, final, initial, medial) ; 0 = forme absente (right-joining :
/// pas d'initial/medial ; hamza : isolated seule ; tatweel : inchange).
fn ar_forms(cp: u32) -> Option<[u32; 4]> {
    Some(match cp {
        0x0621 => [0xFE80, 0, 0, 0],
        0x0622 => [0xFE81, 0xFE82, 0, 0],
        0x0623 => [0xFE83, 0xFE84, 0, 0],
        0x0624 => [0xFE85, 0xFE86, 0, 0],
        0x0625 => [0xFE87, 0xFE88, 0, 0],
        0x0626 => [0xFE89, 0xFE8A, 0xFE8B, 0xFE8C],
        0x0627 => [0xFE8D, 0xFE8E, 0, 0],
        0x0628 => [0xFE8F, 0xFE90, 0xFE91, 0xFE92],
        0x0629 => [0xFE93, 0xFE94, 0, 0],
        0x062A => [0xFE95, 0xFE96, 0xFE97, 0xFE98],
        0x062B => [0xFE99, 0xFE9A, 0xFE9B, 0xFE9C],
        0x062C => [0xFE9D, 0xFE9E, 0xFE9F, 0xFEA0],
        0x062D => [0xFEA1, 0xFEA2, 0xFEA3, 0xFEA4],
        0x062E => [0xFEA5, 0xFEA6, 0xFEA7, 0xFEA8],
        0x062F => [0xFEA9, 0xFEAA, 0, 0],
        0x0630 => [0xFEAB, 0xFEAC, 0, 0],
        0x0631 => [0xFEAD, 0xFEAE, 0, 0],
        0x0632 => [0xFEAF, 0xFEB0, 0, 0],
        0x0633 => [0xFEB1, 0xFEB2, 0xFEB3, 0xFEB4],
        0x0634 => [0xFEB5, 0xFEB6, 0xFEB7, 0xFEB8],
        0x0635 => [0xFEB9, 0xFEBA, 0xFEBB, 0xFEBC],
        0x0636 => [0xFEBD, 0xFEBE, 0xFEBF, 0xFEC0],
        0x0637 => [0xFEC1, 0xFEC2, 0xFEC3, 0xFEC4],
        0x0638 => [0xFEC5, 0xFEC6, 0xFEC7, 0xFEC8],
        0x0639 => [0xFEC9, 0xFECA, 0xFECB, 0xFECC],
        0x063A => [0xFECD, 0xFECE, 0xFECF, 0xFED0],
        0x0640 => [0x0640, 0x0640, 0x0640, 0x0640],
        0x0641 => [0xFED1, 0xFED2, 0xFED3, 0xFED4],
        0x0642 => [0xFED5, 0xFED6, 0xFED7, 0xFED8],
        0x0643 => [0xFED9, 0xFEDA, 0xFEDB, 0xFEDC],
        0x0644 => [0xFEDD, 0xFEDE, 0xFEDF, 0xFEE0],
        0x0645 => [0xFEE1, 0xFEE2, 0xFEE3, 0xFEE4],
        0x0646 => [0xFEE5, 0xFEE6, 0xFEE7, 0xFEE8],
        0x0647 => [0xFEE9, 0xFEEA, 0xFEEB, 0xFEEC],
        0x0648 => [0xFEED, 0xFEEE, 0, 0],
        0x0649 => [0xFEEF, 0xFEF0, 0, 0],
        0x064A => [0xFEF1, 0xFEF2, 0xFEF3, 0xFEF4],
        _ => return None,
    })
}

/// Diacritique combinant : transparent pour la liaison, reste avec sa base.
fn ar_is_mark(cp: u32) -> bool {
    (0x064B..=0x065F).contains(&cp) || cp == 0x0670
}

/// Ligature lam-alef obligatoire : forme isolee (finale = isolee + 1).
fn ar_lam_alef(cp: u32) -> Option<u32> {
    Some(match cp {
        0x0622 => 0xFEF5,
        0x0623 => 0xFEF7,
        0x0625 => 0xFEF9,
        0x0627 => 0xFEFB,
        _ => return None,
    })
}

/// `visual` : inverse chaque run arabe apres shaping (terminal sans BiDi).
fn arabic_display(text: &str, visual: bool) -> String {
    // Fast path : rien a shaper (et idempotence, les U+FExx ne rematchent pas).
    if !text.chars().any(|c| (0x0621..=0x064A).contains(&(c as u32))) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let cp = chars[i] as u32;
        if ar_forms(cp).is_none() && !ar_is_mark(cp) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Run arabe -> clusters (base, marks). Base 0 = diacritique orphelin.
        let mut clusters: Vec<(u32, Vec<u32>)> = Vec::new();
        while i < chars.len() {
            let cp = chars[i] as u32;
            if ar_forms(cp).is_some() {
                clusters.push((cp, Vec::new()));
            } else if ar_is_mark(cp) {
                match clusters.last_mut() {
                    Some(last) => last.1.push(cp),
                    None => clusters.push((0, vec![cp])),
                }
            } else {
                break;
            }
            i += 1;
        }
        // pair(P, L) = P a une forme initiale (dual) ET L une forme finale.
        let links_prev: Vec<bool> = (0..clusters.len())
            .map(|k| {
                k > 0
                    && clusters[k - 1].0 != 0
                    && clusters[k].0 != 0
                    && ar_forms(clusters[k - 1].0).is_some_and(|f| f[2] != 0)
                    && ar_forms(clusters[k].0).is_some_and(|f| f[1] != 0)
            })
            .collect();
        // Formes contextuelles + ligatures, construites en ordre logique. Le
        // run n'est inverse ici que si le terminal hote ne fait pas le BiDi.
        let mut pieces: Vec<String> = Vec::new();
        let mut k = 0;
        while k < clusters.len() {
            let b = clusters[k].0;
            let mut piece = String::new();
            let lam_alef = b == 0x0644
                && k + 1 < clusters.len()
                && ar_lam_alef(clusters[k + 1].0).is_some();
            if lam_alef {
                let mut lig = ar_lam_alef(clusters[k + 1].0).unwrap();
                if links_prev[k] {
                    lig += 1;
                }
                piece.push(char::from_u32(lig).unwrap());
                for &m in clusters[k].1.iter().chain(clusters[k + 1].1.iter()) {
                    piece.push(char::from_u32(m).unwrap());
                }
                k += 2;
            } else if b != 0 {
                let f = ar_forms(b).unwrap();
                let link_n = k + 1 < clusters.len()
                    && f[2] != 0
                    && clusters[k + 1].0 != 0
                    && ar_forms(clusters[k + 1].0).is_some_and(|nf| nf[1] != 0);
                let form = match (links_prev[k], link_n) {
                    (true, true) => f[3],
                    (true, false) => f[1],
                    (false, true) => f[2],
                    (false, false) => f[0],
                };
                piece.push(char::from_u32(form).unwrap());
                for &m in &clusters[k].1 {
                    piece.push(char::from_u32(m).unwrap());
                }
                k += 1;
            } else {
                for &m in &clusters[k].1 {
                    piece.push(char::from_u32(m).unwrap());
                }
                k += 1;
            }
            pieces.push(piece);
        }
        if visual {
            pieces.reverse();
        }
        for piece in &pieces {
            out.push_str(piece);
        }
    }
    out
}

fn is_arabic_display_char(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x06FF | 0xFE70..=0xFEFF)
}

// =================== BUILD LINE 1 (banner) ===================

struct GradStop(u8, u8, u8);

struct BannerSeg {
    text: String,
    fg: String,
}

// Icone de la machine ou tourne claude (pas celle ou sont les mains de l'user :
// en SSH laptop -> fixe, la statusline s'execute sur le fixe et affiche le fixe,
// coherent avec le chemin rendu juste a cote).
//
// Glyphes Nerd Font, en echappement plutot qu'en litteral (plan supplementaire
// UTF-8 4 octets). Codepoints releves dans bin/scripts/lib/i_md.sh du depot
// ryanoasis/nerd-fonts, pas de memoire :
//   U+F01C5 nf-md-desktop_tower -> desktop
//   U+F0322 nf-md-laptop        -> laptop  (idem config fastfetch, install.ps1)
const ICON_DESKTOP: &str = "\u{f01c5}";
const ICON_LAPTOP: &str = "\u{f0322}";

// Detection par presence de batterie, meme signal que install.ps1 pour fastfetch.
// GetSystemPowerStatus est un simple appel kernel32 (pas de WMI, pas de process,
// pas d'I/O) : negligeable face aux dizaines de ms du git du meme tick.
//
// Pourquoi pas un `if hostname == ...` : le repo est partageable, un hostname en
// dur afficherait la mauvaise icone chez quiconque le clone (regle d'or 1 du
// CLAUDE.md du repo).
#[cfg(windows)]
fn device_icon() -> &'static str {
    #[repr(C)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    let mut status = SystemPowerStatus {
        ac_line_status: 0,
        battery_flag: 0,
        battery_life_percent: 0,
        system_status_flag: 0,
        battery_life_time: 0,
        battery_full_life_time: 0,
    };
    let ok = unsafe { GetSystemPowerStatus(&mut status) } != 0;

    // 128 = "No system battery", 255 = "Unknown status". Appel echoue ou statut
    // inconnu -> desktop, le cas majoritaire, plutot qu'une icone laptop fausse.
    if !ok || status.battery_flag == 128 || status.battery_flag == 255 {
        ICON_DESKTOP
    } else {
        ICON_LAPTOP
    }
}

// Le binaire n'est deploye que sur Windows (settings.json -> ~/.claude/statusline.exe).
// Ce cfg existe pour que le crate compile ailleurs, pas pour y etre utilise.
#[cfg(not(windows))]
fn device_icon() -> &'static str {
    ICON_DESKTOP
}

#[allow(clippy::too_many_arguments)]
fn build_line1(
    dir: &str,
    git: &GitInfo,
    mode: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    ctx_pct: Option<f64>,
    ctx_tokens: Option<i64>,
    ctx_size: Option<i64>,
) -> String {
    // Couleur de fin de banner (chevron + dernier stop ou fond uni)
    let (p_r, p_g, p_b, grad_stops): (u8, u8, u8, Option<Vec<GradStop>>) = match mode {
        Some("bypassPermissions") => (255, 121, 198, None),
        Some("plan") => (139, 233, 253, None),
        Some("acceptEdits") => (80, 250, 123, None),
        Some("dontAsk") => (189, 147, 249, None),
        Some("auto") => (255, 184, 108, None),
        _ => {
            let stops = vec![
                GradStop(180, 190, 254),
                GradStop(137, 180, 250),
                GradStop(116, 199, 236),
            ];
            let last = stops.last().unwrap();
            (last.0, last.1, last.2, Some(stops))
        }
    };
    let path_fg = rgb(p_r, p_g, p_b);
    let path_bg = bg(p_r, p_g, p_b);

    // Section 2 (model + ctx)
    let s2 = (60u8, 64u8, 80u8);
    let s2_bg = bg(s2.0, s2.1, s2.2);
    let s2_fg = rgb(220, 220, 220);

    let path_text_fg = rgb(25, 28, 42);
    let chevron = '\u{E0B0}';

    // Construction segments du banner path. L'icone machine est DANS le meme
    // segment que le chemin : elle herite du fond (degrade ou uni selon le mode)
    // et compte dans total_len, donc le degrade per-caractere reste continu.
    // Deux espaces apres le glyphe : les Material Design de Nerd Font sont rendus
    // double-largeur par Windows Terminal, un seul espace les colle au "C:".
    let mut segs: Vec<BannerSeg> = vec![BannerSeg {
        text: format!(" {}  {}", device_icon(), dir),
        fg: path_text_fg.clone(),
    }];

    if let Some(branch) = &git.branch {
        // Texte git unifie avec celui du path : meme couleur sombre sur le fond
        // bleu degrade -- les parentheses suffisent a delimiter le bloc git, pas
        // besoin d'un gris distinct qui creait une 2e teinte sur la meme banniere.
        let branch_fg = path_text_fg.clone();
        // Sync arrows ↑/↓ en violet Copilot (#8534F3, https://brand.github.com/
        // foundations/color) -- couleur signature GitHub, saturee donc visible
        // sur le fond bleu clair du path, sans avoir l'air d'une alerte (sinon
        // ça crierait à chaque commit non poussé). Le jaune fetch_stale reste
        // en alerte distincte (le fetch background est planté = info perimee).
        let branch_sync_fg = if git.fetch_stale { rgb(200, 170, 100) } else { rgb(133, 52, 243) };

        let mut prefix = format!(" ({}", branch);
        if let Some(sha) = &git.sha {
            prefix.push_str(&format!(" {}", sha));
        }
        segs.push(BannerSeg { text: prefix, fg: branch_fg.clone() });
        if git.ahead > 0 {
            segs.push(BannerSeg { text: format!(" \u{2191}{}", git.ahead), fg: branch_sync_fg.clone() });
        }
        if git.behind > 0 {
            segs.push(BannerSeg { text: format!(" \u{2193}{}", git.behind), fg: branch_sync_fg.clone() });
        }
        if git.dirty > 0 {
            segs.push(BannerSeg { text: format!(" *{}", git.dirty), fg: branch_fg.clone() });
        }
        segs.push(BannerSeg { text: ")".to_string(), fg: branch_fg.clone() });
    }
    segs.push(BannerSeg { text: " ".to_string(), fg: path_text_fg.clone() });

    let mut line1 = String::new();

    if let Some(stops) = &grad_stops {
        // Degrade per-character entre stops, interpolation lineaire
        let total_len: usize = segs.iter().map(|s| s.text.chars().count()).sum();
        let seg_count = (stops.len() - 1) as f64;
        let mut idx = 0usize;
        for s in &segs {
            let chars: Vec<char> = s.text.chars().collect();
            let mut j = 0usize;
            while j < chars.len() {
                let u = if total_len > 1 {
                    (idx as f64 / (total_len - 1) as f64) * seg_count
                } else {
                    0.0
                };
                let mut seg = u.floor() as usize;
                if seg >= stops.len() - 1 {
                    seg = stops.len() - 2;
                }
                let t = u - seg as f64;
                let a = &stops[seg];
                let b = &stops[seg + 1];
                let r = (a.0 as f64 + (b.0 as f64 - a.0 as f64) * t).round() as u8;
                let g = (a.1 as f64 + (b.1 as f64 - a.1 as f64) * t).round() as u8;
                let bb = (a.2 as f64 + (b.2 as f64 - a.2 as f64) * t).round() as u8;
                line1.push_str(&bg(r, g, bb));
                line1.push_str(&s.fg);

                if is_arabic_display_char(chars[j]) {
                    // Garder tout le run arabe sous un seul style ANSI. Des SGR
                    // entre chaque lettre fragmentent le run BiDi dans la TUI
                    // Claude et cassent son ordre ainsi que ses liaisons.
                    while j < chars.len() && is_arabic_display_char(chars[j]) {
                        line1.push(chars[j]);
                        j += 1;
                        idx += 1;
                    }
                } else {
                    line1.push(chars[j]);
                    j += 1;
                    idx += 1;
                }
            }
        }
        line1.push_str(RESET);
    } else {
        // Fond uni
        for s in &segs {
            line1.push_str(&path_bg);
            line1.push_str(&s.fg);
            line1.push_str(&s.text);
        }
        line1.push_str(RESET);
    }

    // Transition path -> banner 2 (model + ctx). Section cout ($) retiree :
    // total_cost_usd est un estimatif au tarif API, sans signification en
    // abonnement Pro/Max (forfait fixe, pas de facturation au token). Les vraies
    // jauges de budget sont les barres 5h/7d/opus de la ligne 2.
    line1.push_str(&path_fg);
    line1.push_str(&s2_bg);
    line1.push(chevron);

    // Banner 2 : modele + effort + ctx
    line1.push_str(&s2_fg);
    line1.push(' ');
    if let Some(m) = model {
        line1.push_str(m);
        line1.push_str("  ");
    }

    let effort_str = get_effort_display(effort);
    if !effort_str.is_empty() {
        line1.push_str(&effort_str);
        line1.push_str(RESET);
        line1.push_str(&s2_bg);
        line1.push_str(&s2_fg);
        line1.push_str("  ");
    }

    let ctx_pct_safe = ctx_pct.unwrap_or(0.0);
    let col = get_context_color(ctx_pct_safe);
    match (ctx_tokens, ctx_size) {
        (Some(t), Some(sz)) => {
            line1.push_str(&format!("{}{}/{}{} tok", col, format_tokens(t), format_tokens(sz), s2_fg));
        }
        _ => {
            line1.push_str(&format!("ctx {}{} %{}", col, ctx_pct_safe as i64, s2_fg));
        }
    }
    line1.push(' ');

    // Chevron final
    line1.push_str(RESET);
    line1.push_str(&rgb(s2.0, s2.1, s2.2));
    line1.push(chevron);
    line1.push_str(RESET);

    line1
}

// =================== BUILD LINE 2 (usage) ===================

fn build_usage_seg(label: &str, util: f64, resets_at: &Value, stale: bool, reference: Option<DateTime<Utc>>) -> String {
    // Fenetre EXPIREE : resets_at deja passe (compare a l'heure REELLE Utc::now(),
    // PAS au mtime du cache -- sinon on raterait une fenetre expiree entre le mtime
    // et maintenant). Sa valeur d'usage appartient a la fenetre PRECEDENTE, que la
    // source soit le cache stale OU le stdin de claude : ce dernier continue de
    // rapporter l'ancienne fenetre (ancien %, ancien resets_at deja passe -> "now")
    // tant que claude n'a pas refait d'appel API apres le reset. C'est exactement le
    // "5h 100 % (now)" rouge trompeur signale : on est en realite au debut d'une
    // nouvelle fenetre (~0 %). On neutralise donc au lieu d'afficher une fausse
    // alerte : label + barre grises (derniere valeur connue, jamais en rouge), "—"
    // au lieu du %, et marqueur "(perime <age depuis le reset>)".
    if let Some(reset_utc) = resets_at
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
    {
        let now = Utc::now();
        if reset_utc <= now {
            let grey = rgb(140, 145, 165);
            let bar = format_bar(util, &grey, 14);
            let age = fmt_age(now.signed_duration_since(reset_utc).num_seconds());
            return format!(
                "{}{}{} {} {}\u{2014} (p\u{00E9}rim\u{00E9} {}){}",
                grey, label, RESET, bar, grey, age, RESET
            );
        }
    }

    let col = get_usage_color(util, stale);
    let bar = format_bar(util, &col, 14);
    let mut seg = format!("{}{}{} {} {}{} %{}", col, label, RESET, bar, col, util as i64, RESET);
    if let Some(rst) = format_reset(resets_at, reference) {
        let reset_col = rgb(140, 145, 165);
        seg.push_str(&format!(" {}({}){}", reset_col, rst, RESET));
    }
    seg
}

fn build_line2(usage: &UsageResult) -> String {
    let Some(u) = &usage.json else { return String::new(); };
    let stale = usage.stale;
    let reference = usage.reference;
    let sep = format!(" {}\u{00B7}{} ", rgb(220, 220, 220), RESET);

    let mut segments: Vec<String> = Vec::new();

    if let Some(fh) = u.get("five_hour") {
        if let Some(util) = fh.get("utilization").and_then(|v| v.as_f64()) {
            let resets = fh.get("resets_at").cloned().unwrap_or(Value::Null);
            segments.push(build_usage_seg("5h", util, &resets, stale, reference));
        }
    }
    if let Some(sd) = u.get("seven_day") {
        if let Some(util) = sd.get("utilization").and_then(|v| v.as_f64()) {
            let resets = sd.get("resets_at").cloned().unwrap_or(Value::Null);
            segments.push(build_usage_seg("7d", util, &resets, stale, reference));
        }
    }
    if let Some(sdo) = u.get("seven_day_opus") {
        let util = sdo.get("utilization").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let has_reset = sdo.get("resets_at").and_then(|v| v.as_str()).is_some();
        if util > 0.0 && has_reset {
            let resets = sdo.get("resets_at").cloned().unwrap_or(Value::Null);
            segments.push(build_usage_seg("opus", util, &resets, stale, reference));
        }
    }

    segments.join(&sep)
}

// =================== OLLAMA CLOUD USAGE ===================

// `ollama launch claude` ne modifie pas settings.json : il injecte des variables
// d'environnement dans le process claude (verifie en capturant l'env d'un faux
// claude lance via la commande). La statusline etant un enfant de claude, elle en
// herite. Signal le plus fiable : les modeles par defaut sont mappes sur des cibles
// ":cloud" ; en repli, ANTHROPIC_BASE_URL pointe sur le daemon ollama local
// (127.0.0.1:11434/11435, = OLLAMA_HOST).
fn detect_ollama_env() -> bool {
    let is_cloud = |k: &str| std::env::var(k).map(|v| v.contains(":cloud")).unwrap_or(false);
    if is_cloud("ANTHROPIC_DEFAULT_OPUS_MODEL")
        || is_cloud("ANTHROPIC_DEFAULT_SONNET_MODEL")
        || is_cloud("ANTHROPIC_DEFAULT_HAIKU_MODEL")
        || is_cloud("ANTHROPIC_MODEL")
    {
        return true;
    }
    if let Ok(base) = std::env::var("ANTHROPIC_BASE_URL") {
        let b = base.to_lowercase();
        if b.contains("ollama") || b.contains(":11434") || b.contains(":11435") {
            return true;
        }
        if let Ok(host) = std::env::var("OLLAMA_HOST") {
            if !base.is_empty() && base == host {
                return true;
            }
        }
    }
    false
}

// Detection robuste : claude SCRUBE les ANTHROPIC_* de l'env qu'il passe au
// sous-process statusline (verifie : BASE_URL=None cote statusline), et le model.id
// du stdin reste l'alias "claude-opus-4-8" (pas ":cloud"). MAIS l'environ PROPRE du
// process claude (et de ses ancetres) garde ANTHROPIC_BASE_URL=http://127.0.0.1:1143x
// et ANTHROPIC_DEFAULT_*_MODEL=...:cloud. On remonte donc la chaine des ppid en lisant
// /proc/<pid>/environ jusqu'a trouver le signal (Linux uniquement). Cousin de la
// technique borrow_win_env. Valide : meme avec `env -i`, l'enfant lit l'env du parent.
#[cfg(target_os = "linux")]
fn parent_pid(pid: i32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // comm (champ 2) peut contenir espaces/parentheses -> couper apres le dernier ')'
    let rest = &stat[stat.rfind(')')? + 2..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "linux")]
fn detect_ollama_proc() -> bool {
    let mut pid = std::process::id() as i32;
    for _ in 0..8 {
        if let Ok(env) = fs::read(format!("/proc/{}/environ", pid)) {
            for kv in env.split(|&b| b == 0) {
                if let Ok(s) = std::str::from_utf8(kv) {
                    if let Some(v) = s.strip_prefix("ANTHROPIC_BASE_URL=") {
                        let l = v.to_lowercase();
                        if l.contains("ollama") || l.contains(":11434") || l.contains(":11435") {
                            return true;
                        }
                    }
                    if s.starts_with("ANTHROPIC_DEFAULT_") && s.ends_with(":cloud") {
                        return true;
                    }
                    if let Some(v) = s.strip_prefix("ANTHROPIC_MODEL=") {
                        if v.contains(":cloud") {
                            return true;
                        }
                    }
                }
            }
        }
        match parent_pid(pid) {
            Some(p) if p > 1 => pid = p,
            _ => break,
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
fn detect_ollama_proc() -> bool {
    false
}

// Lit le cache d'usage Ollama (ecrit par ollama-usage.py). Si le cache a > 60 s,
// (re)lance le helper en arriere-plan detache -- meme pattern que le fetch git :
// un marqueur cooldown evite les rafales, et on rend immediatement avec la valeur
// en cache (la prochaine invocation affiche la valeur fraiche). Le scrape lui-meme
// (lecture du cookie Firefox + HTTP vers ollama.com/settings) reste donc hors du
// chemin chaud du binaire 10 Hz.
fn read_ollama_usage(claude_dir: &Path) -> Option<Value> {
    let cache = claude_dir.join("ollama-usage-cache.json");
    let stale = file_age_secs(&cache).map(|a| a >= 60.0).unwrap_or(true);
    if stale {
        let marker = claude_dir.join("ollama-usage-last-fetch");
        let needs = file_age_secs(&marker).map(|a| a >= 15.0).unwrap_or(true);
        if needs {
            touch(&marker);
            let helper = claude_dir.join("ollama-usage.py");
            if helper.exists() {
                let mut cmd = Command::new("python3");
                cmd.arg(&helper);
                cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
                #[cfg(windows)]
                cmd.creation_flags(CREATE_NO_WINDOW | 0x00000008 /* DETACHED_PROCESS */);
                let _ = cmd.spawn();
            }
        }
    }
    let raw = fs::read_to_string(&cache).ok()?;
    serde_json::from_str(&raw).ok()
}

// Construit la ligne 2 en mode Ollama : memes couleurs / barres / format que la
// version Anthropic (cf. build_usage_seg), mais alimentee par session/weekly d'Ollama
// Cloud. Labels "5h"/"7d" : la session Ollama se reinitialise toutes les 5 h et le
// quota hebdomadaire tous les 7 j -- meme semantique que les fenetres Anthropic.
fn build_line2_ollama(u: &Value) -> String {
    let sep = format!(" {}\u{00B7}{} ", rgb(220, 220, 220), RESET);
    let mut segments: Vec<String> = Vec::new();
    for (key, label) in [("session", "5h"), ("weekly", "7d")] {
        if let Some(w) = u.get(key) {
            if let Some(util) = w.get("utilization").and_then(|v| v.as_f64()) {
                let col = get_usage_color(util, false);
                let bar = format_bar(util, &col, 14);
                // pct = chaine exacte affichee par ollama.com (ex. "3.5"), repli sur
                // l'entier tronque si absente. La barre, elle, utilise le float.
                let pct = w
                    .get("pct")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| (util as i64).to_string());
                let mut seg = format!(
                    "{}{}{} {} {}{} %{}",
                    col, label, RESET, bar, col, pct, RESET
                );
                if let Some(rst) = w.get("reset").and_then(|v| v.as_str()) {
                    let reset_col = rgb(140, 145, 165);
                    seg.push_str(&format!(" {}({}){}", reset_col, rst, RESET));
                }
                segments.push(seg);
            }
        }
    }
    segments.join(&sep)
}

// =================== OLLAMA MODEL / CONTEXT WINDOW ===================

// Remonte la chaine des ppid pour extraire le VRAI nom du modele Ollama Cloud
// (ex. "deepseek-v4-pro:cloud") depuis l'environ du process claude parent.
// Claude scrubbe ANTHROPIC_* au spawn du statusline, mais ses ancetres
// conservent les variables (meme technique que detect_ollama_proc).
#[cfg(target_os = "linux")]
fn get_ollama_model() -> Option<String> {
    let mut pid = std::process::id() as i32;
    for _ in 0..8 {
        if let Ok(env) = fs::read(format!("/proc/{}/environ", pid)) {
            for kv in env.split(|&b| b == 0) {
                if let Ok(s) = std::str::from_utf8(kv) {
                    for prefix in [
                        "ANTHROPIC_DEFAULT_OPUS_MODEL=",
                        "ANTHROPIC_DEFAULT_SONNET_MODEL=",
                        "ANTHROPIC_DEFAULT_HAIKU_MODEL=",
                        "ANTHROPIC_MODEL=",
                    ] {
                        if let Some(v) = s.strip_prefix(prefix) {
                            if v.contains(":cloud") {
                                return Some(v.to_string());
                            }
                        }
                    }
                }
            }
        }
        match parent_pid(pid) {
            Some(p) if p > 1 => pid = p,
            _ => break,
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn get_ollama_model() -> Option<String> {
    None
}

// =================== CONTEXT WINDOW (dynamique, via `ollama show` + cache) ===================
//
// Pas de table de fenetres codees en dur : elles etaient peu fiables (ex. la
// table annoncait minimax-m3 a 1M alors que `ollama show` donne 524288, et il
// fallait l'editer + rebuild a chaque nouveau modele). `ollama show` est la
// SEULE source de verite, le resultat est mis en cache disque par modele.

// Cache persistant model -> context length (tokens), rempli a la demande par un
// `ollama show <model>` lance en arriere-plan. But : ne plus maintenir a la main
// la table ollama_context_window() a chaque nouveau modele cloud. Une fois un
// modele resolu, sa fenetre est ecrite ici et relue instantanement aux ticks
// suivants (zero spawn). Format : {"glm-5.2:cloud": 1000000, ...}.
fn ollama_context_cache_path(claude_dir: &Path) -> PathBuf {
    claude_dir.join("ollama-context-cache.json")
}

fn read_cached_context(claude_dir: &Path, model: &str) -> Option<i64> {
    let raw = fs::read_to_string(ollama_context_cache_path(claude_dir)).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v.get(model).and_then(|x| x.as_i64())
}

// Parse la sortie de `ollama show <model>` pour extraire "context length".
// Exemple de ligne (espaces variables) : "    context length      1000000".
fn parse_ollama_context_length(output: &str) -> Option<i64> {
    for line in output.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("context length") {
            let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}

// Nom de fichier sur : un model contient ':' '/' '.' interdits/genants.
fn sanitize_model(model: &str) -> String {
    model
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

// Spawn detache d'une re-invocation de soi-meme (`statusline.exe --resolve-ctx
// <model>`) qui fera le `ollama show` + ecrira le cache. Cooldown par modele
// pour ne pas spammer pendant que la 1ere resolution est en vol (le cache etant
// permanent une fois ecrit, un cooldown court suffit). Meme pattern detache que
// le git fetch / read_ollama_usage : zero blocage du chemin chaud.
fn spawn_context_resolver(claude_dir: &Path, model: &str) {
    let marker = claude_dir.join(format!("ollama-ctx-fetch-{}", sanitize_model(model)));
    let needs = file_age_secs(&marker).map(|a| a >= 30.0).unwrap_or(true);
    if !needs {
        return;
    }
    touch(&marker);
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.arg("--resolve-ctx").arg(model);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW | 0x00000008 /* DETACHED_PROCESS */);
    let _ = cmd.spawn();
}

// Resolution : le cache (rempli par `ollama show`, SEULE source de verite). Si
// le modele n'y est pas encore, on declenche un resolver en arriere-plan et on
// rend None ce tick -- Claude Code affiche alors sa valeur par defaut (~200k) le
// temps d'un tick, puis la vraie fenetre des que le cache est ecrit. Aucune
// valeur codee en dur => jamais de fenetre fausse, et zero maintenance par modele.
fn resolve_ollama_context(claude_dir: &Path, model: &str) -> Option<i64> {
    if let Some(sz) = read_cached_context(claude_dir, model) {
        return Some(sz);
    }
    spawn_context_resolver(claude_dir, model);
    None
}

// Mode resolver (process detache, lance par spawn_context_resolver) : execute
// `ollama show <model>`, parse, merge dans le cache JSON. N'imprime PAS de
// statusline. Best-effort : echoue silencieusement si ollama absent / modele
// inconnu / parse rate.
fn run_context_resolver(claude_dir: &Path, model: &str) {
    let mut cmd = Command::new("ollama");
    cmd.arg("show").arg(model);
    cmd.stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let Ok(out) = cmd.output() else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let Ok(text) = String::from_utf8(out.stdout) else {
        return;
    };
    let Some(ctx) = parse_ollama_context_length(&text) else {
        return;
    };

    // Read-modify-write du cache (temp + rename atomique sur le meme volume).
    let path = ollama_context_cache_path(claude_dir);
    let mut map = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    map.insert(model.to_string(), Value::from(ctx));
    if let Ok(body) = serde_json::to_string(&Value::Object(map)) {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, body.as_bytes()).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

// =================== MAIN ===================

fn main() {
    // Mode resolver detache (cf. spawn_context_resolver) : resout la fenetre de
    // contexte d'un modele Ollama via `ollama show`, l'ecrit dans le cache, et
    // sort sans rien imprimer. Branche avant toute lecture de stdin.
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() >= 3 && argv[1] == "--resolve-ctx" {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let claude_dir = PathBuf::from(&home).join(".claude");
        run_context_resolver(&claude_dir, &argv[2]);
        return;
    }

    // Mode refresh d'usage detache (cf. spawn_usage_refresh) : interroge
    // /api/oauth/usage et met a jour usage-cache.json (notamment seven_day_opus,
    // absent du stdin). N'imprime pas de statusline. Branche avant la lecture stdin.
    if argv.len() >= 2 && argv[1] == "--refresh-usage" {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let claude_dir = PathBuf::from(&home).join(".claude");
        run_usage_refresh(&claude_dir);
        return;
    }

    // Mode diagnostic : imprime l'empreinte du token courant (debug du mecanisme
    // de detection de changement de compte). Ne fait aucun appel reseau.
    if argv.len() >= 2 && argv[1] == "--token-fp" {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let claude_dir = PathBuf::from(&home).join(".claude");
        if let Some(fp) = current_token_fingerprint(&claude_dir) {
            println!("{}", fp);
        }
        return;
    }

    // INSTRUMENTATION (2026-05-20) : freeze ~3s constant signalé par user.
    // On mesure : durée totale, durée git, durée usage (avec breakdown source / api_ms).
    // Log append-only dans `~/.claude/statusline-tick-log.txt`. À retirer après
    // identification de la cause.
    let t_main_start = Instant::now();
    let start_ms_unix = now_ms();

    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok();

    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let claude_dir = PathBuf::from(&home).join(".claude");

    let _ = fs::write(claude_dir.join("statusline-last-input.json"), &raw);

    let data: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);

    let dir = data
        .pointer("/workspace/current_dir")
        .and_then(|v| v.as_str())
        .or_else(|| data.pointer("/cwd").and_then(|v| v.as_str()))
        .map(String::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        });

    let mode = data
        .get("permission_mode")
        .or_else(|| data.get("permissionMode"))
        .and_then(|v| v.as_str())
        .or_else(|| data.pointer("/session/permission_mode").and_then(|v| v.as_str()))
        .map(String::from);

    let mut model = data
        .pointer("/model/display_name")
        .and_then(|v| v.as_str())
        .or_else(|| data.pointer("/model/id").and_then(|v| v.as_str()))
        .map(String::from);

    // model.id brut (pour la detection Ollama : l'env ANTHROPIC_* peut etre scrube
    // par claude au spawn du statusline, mais l'id du modele est toujours dans le stdin).
    let model_id = data.pointer("/model/id").and_then(|v| v.as_str()).map(String::from);

    let effort = data
        .pointer("/effort/level")
        .and_then(|v| v.as_str())
        .map(String::from);

    let ctx_pct = data
        .pointer("/context_window/used_percentage")
        .and_then(|v| v.as_f64());
    let ctx_tokens = data
        .pointer("/context_window/total_input_tokens")
        .and_then(|v| v.as_i64());
    let mut ctx_size = data
        .pointer("/context_window/context_window_size")
        .and_then(|v| v.as_i64());

    // Pre-extract version + rate_limits du stdin pour read_usage (cf. doc
    // officielle https://code.claude.com/docs/en/statusline -- ces champs sont
    // fournis par Claude Code lui-meme, plus fiables que tout appel HTTP).
    let stdin_version = data.get("version").and_then(|v| v.as_str()).map(String::from);
    let stdin_rate_limits = build_usage_from_stdin_rate_limits(&data);

    let t_git = Instant::now();
    let git = compute_git(&dir);
    let git_ms = t_git.elapsed().as_millis();

    let t_usage = Instant::now();
    // Mode Ollama (`ollama launch claude`) : on ne consomme pas le quota Anthropic,
    // donc afficher ses rate_limits serait trompeur. On source l'usage depuis Ollama
    // Cloud (cache rempli par ollama-usage.py) et on n'interroge PAS api.anthropic.com.
    // Detection : env (si propage) OU id du modele stdin contient ":cloud"/"kimi"
    // (signal robuste, independant de la propagation d'env).
    let model_is_cloud = model_id
        .as_deref()
        .or(model.as_deref())
        .map(|m| {
            let l = m.to_lowercase();
            l.contains(":cloud") || l.contains("kimi")
        })
        .unwrap_or(false);
    // 3 signaux : env direct (si non scrube) | /proc des ancetres (claude garde
    // ANTHROPIC_BASE_URL malgre le scrub) | model.id du stdin contenant ":cloud".
    let ollama = detect_ollama_env() || detect_ollama_proc() || model_is_cloud;

    // Correction du contexte affiche : Claude Code envoie ctx_size=200k (sa valeur
    // par defaut interne) meme quand le modele Ollama sous-jacent supporte 1M.
    // On remplace par la vraie taille, et on affiche le vrai nom du modele.
    //
    // Source du nom de modele cloud, par ordre de fiabilite :
    //   1. get_ollama_model() -> /proc des ancetres (Linux : claude scrube
    //      ANTHROPIC_* ET reduit model.id du stdin a l'alias "claude-opus-4-8").
    //   2. model.id / display_name du stdin -> sous Windows /proc n'existe pas,
    //      mais `ollama launch` laisse passer le vrai nom ":cloud" dans le stdin
    //      (ex. "glm-5.2:cloud"), donc le fallback suffit. Sans ce fallback,
    //      get_ollama_model() renvoyait toujours None sous Windows et ctx_size
    //      restait coince a 200k.
    if ollama {
        let cloud_model = get_ollama_model()
            .or_else(|| model_id.clone())
            .or_else(|| model.clone());
        if let Some(om) = cloud_model {
            model = Some(om.clone());
            if let Some(sz) = resolve_ollama_context(&claude_dir, &om) {
                ctx_size = Some(sz);
            }
        }
    }

    let mut usage_src = String::from("Ollama");
    let mut api_status_s = String::from("-");
    let mut api_attempts_v: u8 = 0;
    let mut api_ms_s = String::from("-");
    let line2 = if ollama {
        read_ollama_usage(&claude_dir)
            .map(|u| build_line2_ollama(&u))
            .unwrap_or_default()
    } else {
        let usage = read_usage(&claude_dir, stdin_rate_limits, stdin_version.as_deref());
        usage_src = format!("{:?}", usage.source);
        api_status_s = usage.api_status.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
        api_attempts_v = usage.api_attempts;
        api_ms_s = usage.api_ms.map(|v| v.to_string()).unwrap_or_else(|| "-".to_string());
        build_line2(&usage)
    };
    let usage_ms = t_usage.elapsed().as_millis();

    // Chemin pour l'AFFICHAGE seulement : pre-shaping arabe, en ordre logique
    // ou visuel selon que le terminal hote reordonne le RTL ou non.
    // compute_git() a recu le chemin brut.
    let ar_mode = arabic_mode();
    let dir_display = match ar_mode {
        ArabicMode::Raw => dir.clone(),
        ArabicMode::Logical => arabic_display(&dir, false),
        ArabicMode::Visual => arabic_display(&dir, true),
    };

    let line1 = build_line1(
        &dir_display,
        &git,
        mode.as_deref(),
        model.as_deref(),
        effort.as_deref(),
        ctx_pct,
        ctx_tokens,
        ctx_size,
    );

    let mut out = line1;
    if !line2.is_empty() {
        out.push_str("\n\n");
        out.push_str(&line2);
    }

    // Print sans newline (comme Write-Host -NoNewline en PowerShell)
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    let _ = h.write_all(out.as_bytes());
    drop(h);

    // INSTRUMENTATION (2026-05-20) : log append-only, best-effort. Le total_ms
    // est mesuré AVANT cette écriture pour ne pas se compter soi-même.
    let total_ms = t_main_start.elapsed().as_millis();
    let _ = (|| -> std::io::Result<()> {
        let log_path = claude_dir.join("statusline-tick-log.txt");
        // Rotation : a 1 Hz x N sessions le log grossit de dizaines de Mo/jour.
        // Au-dela de 8 Mo on bascule vers .old (ecrase le precedent .old) --
        // garde toujours les ~derniers jours sans croissance infinie.
        const LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
        if fs::metadata(&log_path).map(|m| m.len() > LOG_MAX_BYTES).unwrap_or(false) {
            let old = claude_dir.join("statusline-tick-log.old.txt");
            let _ = fs::remove_file(&old);
            let _ = fs::rename(&log_path, &old);
        }
        let line = format!(
            "{} tick={} effort={} bidi={} git_ms={} usage_ms={} usage_src={} api_status={} api_attempts={} api_ms={} total_ms={} pid={}\n",
            start_ms_unix,
            picker_tick(start_ms_unix),
            effort.as_deref().unwrap_or("none"),
            // Ordre d'emission du run arabe effectivement choisi : verifiable
            // depuis un vrai spawn par claude.exe, sans avoir a lire l'env.
            ar_mode.as_str(),
            git_ms,
            usage_ms,
            usage_src,
            api_status_s,
            api_attempts_v,
            api_ms_s,
            total_ms,
            std::process::id(),
        );
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        // Write all bytes en un seul appel : OS append est atomic-by-write
        // sur Windows quand le buffer < PIPE_BUF (~4KB) — pas d'interleaving
        // entre processes parallèles.
        f.write_all(line.as_bytes())?;
        Ok(())
    })();
}

#[cfg(test)]
mod tests {
    use super::{arabic_display, format_bar, parse_ollama_context_length, RESET};

    // Parse de la vraie sortie `ollama show glm-5.2:cloud`.
    #[test]
    fn parse_context_length_reel() {
        let out = "  Model\n    architecture        glm5.2          \n    parameters          756162687872    \n    context length      1000000         \n    embedding length    0               \n    quantization                        \n\n  Capabilities\n    thinking      \n    completion    \n    tools         \n";
        assert_eq!(parse_ollama_context_length(out), Some(1_000_000));
    }

    #[test]
    fn parse_context_length_absent() {
        let out = "  Model\n    architecture        foo\n    parameters          123\n";
        assert_eq!(parse_ollama_context_length(out), None);
    }

    fn s(cps: &[u32]) -> String {
        cps.iter().map(|&c| char::from_u32(c).unwrap()).collect()
    }

    #[test]
    fn ascii_inchange() {
        assert_eq!(arabic_display(r"C:\dev\dev-environment", false), r"C:\dev\dev-environment");
        assert_eq!(arabic_display(r"C:\dev\dev-environment", true), r"C:\dev\dev-environment");
    }

    // al-islam (nom du vault) : memes vecteurs que test-arabic-display.ps1.
    #[test]
    fn al_islam_deux_ligatures() {
        let input = s(&[0x0627, 0x0644, 0x0625, 0x0633, 0x0644, 0x0627, 0x0645]);
        let want = s(&[0xFE8D, 0xFEF9, 0xFEB3, 0xFEFC, 0xFEE1]);
        assert_eq!(arabic_display(&input, false), want);
    }

    // Terminal sans BiDi (Warp) : meme shaping, run pose en ordre visuel --
    // le mim final se retrouve a gauche, l'alef initial a droite.
    #[test]
    fn al_islam_ordre_visuel() {
        let input = s(&[0x0627, 0x0644, 0x0625, 0x0633, 0x0644, 0x0627, 0x0645]);
        let want = s(&[0xFEE1, 0xFEFC, 0xFEB3, 0xFEF9, 0xFE8D]);
        assert_eq!(arabic_display(&input, true), want);
    }

    #[test]
    fn chemin_mixte_vault() {
        let input = format!(r"C:\obsidian-vaults\{}", s(&[0x0627, 0x0644, 0x0625, 0x0633, 0x0644, 0x0627, 0x0645]));
        let want = format!(r"C:\obsidian-vaults\{}", s(&[0xFE8D, 0xFEF9, 0xFEB3, 0xFEFC, 0xFEE1]));
        assert_eq!(arabic_display(&input, false), want);
    }

    // L'inversion ne doit toucher que le run arabe : le prefixe ASCII du
    // chemin (lettre de lecteur, separateurs) reste en place.
    #[test]
    fn chemin_mixte_vault_visuel() {
        let input = format!(r"C:\obsidian-vaults\{}", s(&[0x0627, 0x0644, 0x0625, 0x0633, 0x0644, 0x0627, 0x0645]));
        let want = format!(r"C:\obsidian-vaults\{}", s(&[0xFEE1, 0xFEFC, 0xFEB3, 0xFEF9, 0xFE8D]));
        assert_eq!(arabic_display(&input, true), want);
    }

    #[test]
    fn marhaban_right_joiners() {
        let input = s(&[0x0645, 0x0631, 0x062D, 0x0628, 0x0627]);
        let want = s(&[0xFEE3, 0xFEAE, 0xFEA3, 0xFE92, 0xFE8E]);
        assert_eq!(arabic_display(&input, false), want);
        let want_visuel = s(&[0xFE8E, 0xFE92, 0xFEA3, 0xFEAE, 0xFEE3]);
        assert_eq!(arabic_display(&input, true), want_visuel);
    }

    #[test]
    fn idempotence() {
        let shaped = s(&[0xFE8D, 0xFEF9, 0xFEB3, 0xFEFC, 0xFEE1]);
        assert_eq!(arabic_display(&shaped, false), shaped);
        assert_eq!(arabic_display(&shaped, true), shaped);
    }

    #[test]
    fn barre_continue_box_drawing() {
        let plain = format_bar(50.0, "", 4).replace(RESET, "");
        assert!(plain.starts_with("\u{2501}\u{2501}"));
        assert!(!plain.contains('\u{25AC}'));
    }

    use super::{build_usage_seg, fmt_age};
    use serde_json::Value;

    #[test]
    fn fmt_age_paliers() {
        assert_eq!(fmt_age(-5), "0s");
        assert_eq!(fmt_age(30), "30s");
        assert_eq!(fmt_age(90), "1m");
        assert_eq!(fmt_age(3600), "1h");
        assert_eq!(fmt_age(90_000), "1j");
    }

    // Fenetre dont resets_at est dans le passe (2020 < maintenant) : neutralisee.
    // On ne doit JAMAIS voir le "100 %" ni le "(now)" rouge, mais le marqueur perime.
    #[test]
    fn usage_seg_expiree_neutralise() {
        let past = Value::from("2020-01-01T00:00:00+00:00");
        let seg = build_usage_seg("5h", 100.0, &past, false, None);
        assert!(seg.contains("p\u{00E9}rim\u{00E9}"), "marqueur perime attendu");
        assert!(!seg.contains("100 %"), "le pourcentage stale ne doit pas s'afficher");
        assert!(!seg.contains("(now)"), "pas de (now) trompeur");
    }

    // Fenetre encore valide (resets_at en 2099) : affichage normal du pourcentage,
    // pas de marqueur perime. Vrai aussi pour une donnee stale non expiree.
    #[test]
    fn usage_seg_valide_affiche_pct() {
        let future = Value::from("2099-01-01T00:00:00+00:00");
        let seg = build_usage_seg("5h", 42.0, &future, false, None);
        assert!(seg.contains("42 %"), "pourcentage courant attendu");
        assert!(!seg.contains("p\u{00E9}rim\u{00E9}"), "pas de marqueur perime sur fenetre valide");
    }

    // ---- merge stdin <-> cache API (fix "perime" fige sur session idle) ----

    use super::merge_usage_windows;
    use chrono::{TimeZone, Utc};
    use serde_json::json;

    fn test_now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 16, 12, 0, 0).unwrap()
    }

    #[test]
    fn merge_stdin_vivant_prioritaire() {
        let stdin = json!({"five_hour": {"utilization": 42.0, "resets_at": "2026-07-16T14:00:00+00:00"}});
        let cache = json!({"five_hour": {"utilization": 99.0, "resets_at": "2026-07-16T15:00:00+00:00"}});
        let (m, from_cache) = merge_usage_windows(stdin, Some(&cache), test_now());
        assert_eq!(m["five_hour"]["utilization"], 42.0, "stdin vivant reste autoritaire");
        assert!(!from_cache);
    }

    #[test]
    fn merge_stdin_expire_cache_vivant() {
        let stdin = json!({
            "five_hour": {"utilization": 100.0, "resets_at": "2026-07-16T10:00:00+00:00"},
            "seven_day": {"utilization": 7.0,  "resets_at": "2026-07-20T00:00:00+00:00"}
        });
        let cache = json!({"five_hour": {"utilization": 3.0, "resets_at": "2026-07-16T16:30:00+00:00"}});
        let (m, from_cache) = merge_usage_windows(stdin, Some(&cache), test_now());
        assert_eq!(m["five_hour"]["utilization"], 3.0, "fenetre expiree remplacee par le cache vivant");
        assert_eq!(m["five_hour"]["resets_at"], "2026-07-16T16:30:00+00:00");
        assert_eq!(m["seven_day"]["utilization"], 7.0, "fenetre stdin vivante conservee");
        assert!(from_cache);
    }

    #[test]
    fn merge_fenetre_close_api_posterieure_donne_zero() {
        // L'API a repondu APRES le reset sans renvoyer de five_hour vivante
        // -> fenetre reellement close (aucune session ne l'a redemarree) : 0 %.
        let stdin = json!({"five_hour": {"utilization": 100.0, "resets_at": "2026-07-16T10:00:00+00:00"}});
        let cache = json!({"fetched_at": "2026-07-16T11:58:00+00:00"});
        let (m, from_cache) = merge_usage_windows(stdin, Some(&cache), test_now());
        assert_eq!(m["five_hour"]["utilization"], 0.0);
        assert!(m["five_hour"].get("resets_at").is_none(), "pas de resets_at synthetique");
        assert!(from_cache);
    }

    #[test]
    fn merge_stdin_expire_cache_muet_reste_stdin() {
        // fetch anterieur au reset : on ne sait rien de mieux -> stdin conserve,
        // l'affichage neutralise en "perime" (le refresh API suivant corrigera).
        let stdin = json!({"five_hour": {"utilization": 100.0, "resets_at": "2026-07-16T10:00:00+00:00"}});
        let cache = json!({"fetched_at": "2026-07-16T09:00:00+00:00"});
        let (m, from_cache) = merge_usage_windows(stdin, Some(&cache), test_now());
        assert_eq!(m["five_hour"]["utilization"], 100.0);
        assert_eq!(m["five_hour"]["resets_at"], "2026-07-16T10:00:00+00:00");
        assert!(!from_cache);
    }

    #[test]
    fn merge_opus_et_fetched_at_recopies() {
        let stdin = json!({"five_hour": {"utilization": 11.0, "resets_at": "2026-07-16T14:00:00+00:00"}});
        let cache = json!({
            "seven_day_opus": {"utilization": 5.0, "resets_at": "2026-07-20T00:00:00+00:00"},
            "fetched_at": "2026-07-16T11:59:00+00:00"
        });
        let (m, _) = merge_usage_windows(stdin, Some(&cache), test_now());
        assert_eq!(m["seven_day_opus"]["utilization"], 5.0);
        assert_eq!(m["fetched_at"], "2026-07-16T11:59:00+00:00", "date du dernier fetch API preservee");
    }

    #[test]
    fn merge_sans_cache_stdin_intact() {
        let stdin = json!({"five_hour": {"utilization": 100.0, "resets_at": "2026-07-16T10:00:00+00:00"}});
        let (m, from_cache) = merge_usage_windows(stdin.clone(), None, test_now());
        assert_eq!(m, stdin);
        assert!(!from_cache);
    }

    // touch() est la cle de voute des cooldowns (git fetch 30 s, refresh usage
    // 55 s, ollama 15 s) : si le mtime ne bouge pas, CHAQUE tick re-spawn le
    // travail "cooldowne" -> spam (429 chronique constate le 2026-07-17, le
    // marqueur usage-refresh-last etait fige au 25 juin). Piege NTFS : re-creer
    // (truncate) un fichier VIDE n'ecrit aucun octet et ne bump pas
    // LastWriteTime. touch() doit donc ecrire des donnees.
    #[test]
    fn touch_bumpe_mtime_fichier_vide_existant() {
        use std::time::{Duration, SystemTime};
        let path = std::env::temp_dir().join("statusline-touch-test-marker");
        let _ = std::fs::remove_file(&path);
        std::fs::File::create(&path).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(600)).unwrap();
        drop(f);
        let age_before = super::file_age_secs(&path).unwrap();
        assert!(age_before > 500.0, "pre-condition : marqueur backdate ({age_before}s)");
        super::touch(&path);
        let age_after = super::file_age_secs(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(
            age_after < 60.0,
            "touch() doit rajeunir le mtime d'un marqueur vide existant (age: {age_after}s)"
        );
    }

    #[test]
    fn merge_stdin_sans_fenetre_cache_vivant_ajoute() {
        let stdin = json!({"five_hour": {"utilization": 11.0, "resets_at": "2026-07-16T14:00:00+00:00"}});
        let cache = json!({"seven_day": {"utilization": 9.0, "resets_at": "2026-07-20T00:00:00+00:00"}});
        let (m, from_cache) = merge_usage_windows(stdin, Some(&cache), test_now());
        assert_eq!(m["seven_day"]["utilization"], 9.0, "fenetre absente du stdin completee par le cache vivant");
        assert!(from_cache);
    }
}
