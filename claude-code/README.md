# claude-code/

Claude Code config shared between Windows and Android. Single source of truth.

## Contents

| File | Role |
| --- | --- |
| `statusline.ps1` | PowerShell status line: colored path (depends on permission mode), git branch, live 5h/7d usage, plan + email. Portable fallback, no prerequisites. |
| `statusline-rs/` | Rust source for the compiled statusline (xhigh magenta halo + max rainbow stretch). **Requires SAC disabled** — see dedicated section. |
| `settings.json` | Claude Code config (model, plugins, `.exe` statusline) |
| `hooks/` | `auto-pull.ps1`, `auto-push.ps1`, `resolve-sync-conflicts.ps1` — reserved for manual/future use, no longer called from SessionStart/End |
| `hooks/error-vault-trigger.ps1` | `UserPromptSubmit` hook: detects a correction/repetition phrase in the prompt and injects a reminder to consult the `error-vault` skill. Never blocks. |
| `hooks/curfew.ps1` | `UserPromptSubmit` hook: warns (never refuses) on prompts around a time window read from a local, never-committed config; `#jedors` in a prompt buys a silent night. No config file → inert. See the Curfew section below. |
| `hooks/tests/curfew.tests.ps1` | Dependency-free test suite for the curfew hook (`pwsh -NoProfile -File claude-code/hooks/tests/curfew.tests.ps1`). Subfolders of `hooks/` are not deployed by `deploy.ps1`, so tests stay in the repo. |
| `skills/` | Custom skills: claude-file-recovery, copy-edit, css-layout-check, deploy-safety, humanizer, lucide-icons, root-cause-fix, smart-edit, sticky-column-bleed-fix, webapp-deploy |
| `deploy.ps1` | Manual bidirectional sync between this folder and the **Windows** `~/.claude/` |
| `keybindings.json` | Custom Claude Code keybindings: disables Claude Code's native `Alt+V` image paste on Windows so AutoHotkey can paste a local file path instead; also includes `Alt+L` -> `/login` fallback (see Login / Resume shortcuts below) |
| `tmux.conf` | tmux forward 24-bit color so Claude Code renders truecolor over `mosh`+`tmux` (see the SSH Android guide) |
| `scripts/kimi-vision.ps1` | Vision delegation for no-vision backends (e.g. `glm-5.2:cloud`, 1M context but no image support): base64-encodes an image, POSTs it to the local Ollama API (`kimi-k2.7-code:cloud`, MoonViT) and prints the textual description on stdout. Called by the assistant **instead of** `Read` on any image, so the conversation doesn't crash with `API Error: 400 this model does not support image input`. See the dedicated section below. |
| `scripts/screenshot-shortcut.ahk` | AutoHotkey v2 hotkey `Alt+V`: saves a clipboard image to `%USERPROFILE%\Pictures\Screenshots` or finds the latest existing screenshot, then types its absolute path directly with `SendText`. The path is also kept on the clipboard as a fallback. This bypasses Warp/Claude Code's `Ctrl+V` handling. Self-contained, `#SingleInstance Force`, UTF-8. |
| `scripts/latest-screenshot.ps1` | Install-free fallback for the same shortcut (no AutoHotkey): finds the most recent screenshot, copies its path to the clipboard, and with `-Paste` simulates `Ctrl+V`. Bind it to a `.lnk` Shortcut key (e.g. `Ctrl+Alt+V`) if you want the shortcut without installing AHK. |
| `scripts/install-screenshot-shortcut.ps1` | Idempotent bootstrap for the `Alt+V` shortcut: ensures AutoHotkey v2 is installed (`winget --scope user`, no UAC), creates the Startup `.lnk` so AHK relaunches at boot, and launches the `.ahk` immediately. `-Uninstall` removes the `.lnk` and kills the AHK process (leaves AHK and the `.ahk` in place). The Windows bootstrap runs it automatically after `deploy.ps1 -Pull`. |
| `termux/img2clip` | Termux (Android) script: stages a phone photo/screenshot on a PC by uploading it over **SFTP** to `%USERPROFILE%\.claude-images`. SFTP is a binary protocol, so no remote shell parses the transfer — a remote command would break the moment `HKLM\SOFTWARE\OpenSSH\DefaultShell` is PowerShell, which silently interpolates every `$variable` out of the command line (root cause of a 19-day outage, 2026-08-08). With several PCs listed in `WIN_HOSTS`, one SFTP probe per machine reads `.claude-images/.watcher-alive` and the image goes to the machine whose watcher is beating, i.e. the one running a Claude session; falls back to the first reachable machine. The Windows watcher `img-clip-watcher.ps1` (launched beside `claude()` / `ollama()`, **only when `%USERPROFILE%\.claude-images` exists** — create the folder to opt in) detects the file and fills the clipboard in the reader's window station. It deliberately never runs `ssh -p 2222 ... SetImage`: that writes to an ephemeral SSH window station and returns a false success. **Always called with a file-path argument** — by `screenshot-watcher` (auto) and by the `termux-file-editor` share hook. Notifications reuse the `img2clip` id and finish with `Ready to paste`; failures preserve a non-zero exit code so the watcher retries and never counts a failed upload as sent. |
| `termux/termux-file-editor` | Termux built-in hook (deployed to `~/bin/`): triggered by **Share → Termux → EDIT** on any image, delegates to `img2clip`. Requires the Android permission *Display over other apps* on Termux. Lives on the phone, not synced by `deploy.sh` — installed via the express block in the SSH Android guide. |
| `termux/screenshot-watcher` | Polling watcher (every 0.5s while enabled) on the phone's image dirs. As soon as a new screenshot/photo is detected, it posts `Image detected` on the same `img2clip` notification id, then waits until the file size is stable before handing it to `img2clip`; the notification is later replaced in place by transfer progress and final clipboard state. Two streams, both **ON by default** (flags created by `install.sh`, persistent in `$HOME`): `~/.screenshot-watcher.on` = `dcim/Screenshots`, `~/.screenshot-watcher.photos` = `dcim/Camera`. Only the **most recent** new image per stream is staged (not a FIFO replay): the clipboard holds one image, so converging to the newest avoids stale uploads on slow links. The cursor only advances on success, so a failed upload is retried, never silently dropped. Polling vs inotify because FUSE on `/sdcard` doesn't deliver inotify events from other apps. Toggle without killing: `touch`/`rm` the flag (aliases `photos-on`/`photos-off`). Deployed to `~/bin/screenshot-watcher`. |
| `termux/boot-screenshot-watcher` | Termux:Boot wrapper that starts `screenshot-watcher` at phone boot (acquires `termux-wake-lock` first). Deployed to `~/.termux/boot/screenshot-watcher` on the phone — Termux:Boot APK from F-Droid required. |
| `termux/watcher-toggle` | **Persistent** Termux:API notification (`--ongoing` + `--on-delete`) titled `Images to PC`, with a plain state line such as `Screenshots on - Photos paused` and 2 action buttons (`Pause/Enable screenshots`, `Pause/Enable photos`). The line is honest about liveness: if the supervisor/watcher is down it says `Service offline - open Termux`; if images still run but `sshd` is down it appends `ssh phone offline`. If HyperOS still lets the user swipe it away, the delete action calls `watcher-toggle restore` and reposts the same notification immediately; remaining latency is the Android/Termux:API dispatch cost. Flips the same flags as `screenshot-watcher` / the `photos-on`/`photos-off` aliases, so the change takes effect on the next 0.5s watcher pass without restarting it. Buttons re-invoke the script via an **absolute `bash` + absolute script path** (the notification-action env is minimal, like Termux:Boot — no relative resolution possible). Posted on its **own notification channel** and Android notification group `watcher-control` (`Image-control` — no spaces, since `termux-notification-channel` truncates a spaced name to its first word; created best-effort in `show()`), apart from the image notifications. Termux:API does not expose native notification animations, so the UI stays stable and relies on Android's own expand/update motion instead of fake repost animations. Falls back to the default channel (no `--channel`) if channel creation fails, since an invalid channel would drop the notification (per `--help`) — so the notif is never lost. Posted by `install.sh` and re-posted on every boot by `boot-screenshot-watcher`. Silent no-op if Termux:API is absent (flags still controllable from the CLI). Deployed to `~/bin/watcher-toggle`. |

## Bootstrap a new machine

```powershell
# Clone the repo, then pull configs to ~/.claude/
git clone https://github.com/ahmed-mili/dev-environment.git C:\dev\dev-environment
C:\dev\dev-environment\claude-code\deploy.ps1 -Pull
# Restart Claude Code
```

## Daily workflow

Sync between `~/.claude/` and this repo is **manual** (no more auto hooks on SessionStart/End). Rationale: avoid silent Syncthing/git conflicts and keep explicit control over what gets committed.

### You added or modified a skill or setting in `~/.claude/`?

```powershell
cd C:\dev\dev-environment
.\claude-code\deploy.ps1 -Push
git add -A ; git commit -m "<message>" ; git push
```

> ⚠️ `deploy.ps1 -Push` only copies the **11 whitelisted custom skills** to the repo. Anthropic's official skills or marketplace downloads (`brand-guidelines`, `claude-api`, `docx`, etc.) are never committed (keeps the repo lean).

### Device context detection (so the assistant knows where it's talking)

Each shell profile defines a `claude()` wrapper that calls a small detector before launching the real Claude binary. The detector writes a JSON file at `~/.claude/.device-context`:

| Field | Example | Meaning |
| --- | --- | --- |
| `device` | `desktop` / `phone` | Which physical device |
| `context` | `termux` / `pwsh-native` / `ssh-to-desktop` | Which shell / path |
| `shell` | `bash` / `pwsh` | Shell type |
| `model` | `Xiaomi 13T Pro` | Phone model (Termux only) |
| `ssh_from` | `100.x.y.z` | Client IP if connected via SSH |
| `timestamp` | ISO 8601 | When the context was last written |

**Windows pwsh** : the `claude()` function in `$PROFILE` calls `detect.ps1` then the binary.
**Termux** : the `claude()` wrapper in `~/.bashrc` calls `detect.sh` then `claude`.

The assistant can read this file with `Read ~/.claude/.device-context` to know whether the user is on their phone, their PC, or SSH-ing from one to the other. This avoids proposing PC-only actions when the user is on Termux, or phone-only actions when the user is on their desktop.

### Plugin integrity check

Plugins listed in `settings.json` -> `enabledPlugins` may appear "enabled" but not actually installed (cache corruption, new machine, reinstall). This makes skills appear in the system reminder but they are not invocable — the assistant will not know they exist.

**Detect missing plugins:**
```bash
# Termux
bash ~/.claude/scripts/check-plugins.sh

# Windows PowerShell
& ~/.claude/scripts/check-plugins.ps1
```

**Install missing plugins automatically:**
```bash
bash ~/.claude/scripts/check-plugins.sh --fix
# Windows: & ~/.claude/scripts/check-plugins.ps1 -Fix
```

Run this after any `deploy --pull` or `bootstrap` on a new machine, or whenever the Skill Registry looks incomplete. The script parses `settings.json`, compares against `claude plugin list`, and installs whatever is missing.

### To add a new custom skill to the whitelist

Edit `deploy.ps1` line `$CustomSkills = @(...)` and add your skill's folder name.


### One-time bashrc snippet (not auto-synced)

Claude Code downgrades to 256-color whenever it sees `$TMUX` (it ignores `COLORTERM` and `FORCE_COLOR` in that case). A small wrapper in `~/.bashrc` makes `claude` always launch without `TMUX`, so it emits real 24-bit again — `tmux.conf` then forwards it untouched. **Zellij does not trigger this** (it sets `$ZELLIJ`, not `$TMUX`), so truecolor works out of the box there; the wrapper is kept anyway for when `claude` runs inside a real tmux (agent orchestrators). Add this once on a fresh Linux machine (Termux or autre) :

```bash
cat >> ~/.bashrc <<'EOF'

# Claude Code rabaisse en 256-color quand $TMUX est defini -> le lancer sans.
claude() { env -u TMUX claude "$@"; }
EOF
```

(`.bashrc` isn't tracked because each machine's bashrc has unrelated history; the snippet is idempotent — running it twice just defines the function twice, harmless.)

### Shell autosuggestions: ble.sh + atuin + zoxide (opt-in, not auto-synced)

PSReadLine-style inline "ghost text" autosuggestions, fuzzy history search and a
frecency `cd`, so Termux feels like the Windows PowerShell 7 profile:

| Tool | Role | Install |
| --- | --- | --- |
| [ble.sh](https://github.com/akinomyoga/ble.sh) | Line editor: grey ghost-text autosuggestion, syntax highlight, completion menu | `git clone --recursive --depth 1 --shallow-submodules https://github.com/akinomyoga/ble.sh ~/ble.sh && make -C ~/ble.sh && bash ~/ble.sh/ble.sh --install ~/.local/share` |
| [atuin](https://atuin.sh) | History DB + `Ctrl+R` fuzzy search | `curl --proto '=https' --tlsv1.2 -LsSf https://setup.atuin.sh \| sh` |
| [zoxide](https://github.com/ajeetdsouza/zoxide) | `cd` by frecency | `curl -sSfL https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh \| sh` |

Then add this to `~/.bashrc`. Order matters: ble.sh is sourced **early** with
`--attach=none`, and `ble-attach` is the **last** statement of the file.

```bash
# ── ble.sh: line editor. Source EARLY with --attach=none (ble-attach goes last).
if [[ $- == *i* ]] && [[ -f ~/.local/share/blesh/ble.sh ]]; then
  source ~/.local/share/blesh/ble.sh --attach=none
fi

# ... (the rest of your ~/.bashrc) ...

# ── atuin: history DB. --disable-up-arrow leaves the Up key to ble.sh (history
#    + ghost text); Ctrl+R stays atuin's fuzzy-search UI.
. "$HOME/.atuin/bin/env"
eval "$(atuin init bash --disable-up-arrow)"

# atuin registers itself as ble.sh's inline-suggestion source by default. Remove
# it so the ghost text comes from ~/.bash_history (clean) rather than the atuin
# DB — which gets polluted by the multi-line commands Claude Code records via the
# `atuin hook claude-code` hook (parasitic multi-line suggestions on `cd`).
[[ ${BLE_VERSION-} ]] && ble/util/import/eval-after-load core-complete '
    ble/array#remove _ble_complete_auto_source atuin-history'

# ── zoxide: smarter `cd` (frecency). Needs the ble.sh integration module, else
#    ble.sh short-circuits zoxide's `cd` function → "cd: too many arguments".
eval "$(zoxide init bash --cmd cd)"
[[ ${BLE_VERSION-} ]] && ble-import contrib/integration/zoxide

# ── ble.sh: PSReadLine-style behaviour ──
if [[ ${BLE_VERSION-} ]]; then
  ble-face auto_complete='fg=#6C7086'   # grey ghost text, no background
  ble-face syntax_error='fg=default'    # don't paint unknown commands red
  ble-bind -m auto_complete -f 'TAB' 'auto_complete/@end insert'  # Tab accepts
  ble-bind -m auto_complete -f 'C-i' 'auto_complete/@end insert'  # the suggestion
  ble-bind -f 'f3' 'menu-complete'      # F3 = navigable completion menu
  # F2 = sessionizer (menu fzf natif Windows, appele via SSH depuis le tel).
  # -c lance un programme plein ecran externe (fzf) avec le terminal restaure,
  # puis redessine le prompt.
  ble-bind -c 'f2' "$HOME/dev/dev-environment/claude-code/tmux-sessionizer.sh"
fi

# ble-attach MUST be the last statement of ~/.bashrc.
[[ ${BLE_VERSION-} ]] && ble-attach
```

#### Optional: feed Claude Code's own commands into atuin

To record the `Bash` commands Claude Code runs into your atuin history, add these
hooks to `~/.claude/settings.json`. They are kept **out** of the shared
`settings.json` on purpose: without atuin installed, every Bash call would fail
with `atuin: command not found`.

```json
"hooks": {
  "PreToolUse":        [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "atuin hook claude-code" }] }],
  "PostToolUse":       [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "atuin hook claude-code" }] }],
  "PostToolUseFailure":[{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "atuin hook claude-code" }] }]
}
```

> The bashrc snippet above intentionally drops atuin as ble.sh's *inline* ghost-text
> source (the multi-line commands Claude records would otherwise pollute the
> suggestion on `cd`). `Ctrl+R` still searches the full atuin DB, Claude's commands included.

### Phone: continuous block bars in Termux (one-time font patch, not synced)

The `pc` sessionizer menu draws fzf's gutter as one `▌` (U+2588 family) per line. On the desktop they tile into a solid vertical bar; in **Termux** the same bytes render with a ~1px gap between rows. The cause is **Termux's sub-pixel rounding**, *not* the font: JetBrainsMono Nerd Font's block glyphs already fill the cell exactly (`yMax == ascent`, `yMin == descent`), so there's no overshoot to absorb the rounding. (Swapping the font is the wrong fix — verified with fontTools.)

The fix overshoots the **full-height block glyphs** so they bleed past the cell (like DejaVu Sans Mono, whose blocks overshoot by ~25 units and never gap). Patch the phone's `~/.termux/font.ttf` once, on the desktop, then `scp` it back — the family name is unchanged, so Nerd Font icons are untouched:

```bash
pip install --user --break-system-packages fonttools   # PEP 668 override, user-site only
scp phone:~/.termux/font.ttf /tmp/f.ttf                 # thin-client: patch on desktop, not phone
python3 - /tmp/f.ttf /tmp/f-patched.ttf <<'PY'
import sys
from fontTools.ttLib import TTFont
src, dst = sys.argv[1], sys.argv[2]
f = TTFont(src); asc, desc = f['hhea'].ascent, f['hhea'].descent
cmap, glyf = f.getBestCmap(), f['glyf']
DELTA = 64   # overshoot top+bottom; bump to 96/128 if a residual gap remains
for cp in (0x2588,0x2589,0x258A,0x258B,0x258C,0x258D,0x258E,0x258F,0x2590,0x2595):
    gn = cmap.get(cp)
    if not gn or gn not in glyf: continue
    g = glyf[gn]
    if g.isComposite() or g.numberOfContours < 1: continue
    if abs(g.yMax-asc) > 2 or abs(g.yMin-desc) > 2: continue   # full-height bars only
    for i,(x,y) in enumerate(g.coordinates):
        if   y >= g.yMax-1: g.coordinates[i] = (x, y+DELTA)
        elif y <= g.yMin+1: g.coordinates[i] = (x, y-DELTA)
    g.recalcBounds(glyf)
f.save(dst)
PY
ssh phone 'cp ~/.termux/font.ttf ~/.termux/font.ttf.bak'  # backup -> rollback in one line
scp /tmp/f-patched.ttf phone:~/.termux/font.ttf
ssh phone 'termux-reload-settings'
```

Rollback: `ssh phone 'cp ~/.termux/font.ttf.bak ~/.termux/font.ttf && termux-reload-settings'`. Re-run the patch if a Termux font update overwrites it.

## Curfew — warn about new prompts inside a time window

`hooks/curfew.ps1` is a `UserPromptSubmit` hook that **warns** on prompts submitted around a time window you choose (e.g. "no new work between 23:00 and 05:00"). It never refuses anything: it exits 0 and prints a JSON payload whose `systemMessage` you see and whose `additionalContext` tells Claude to wrap up.

Three regimes: **notice** (default 15 / 10 / 5 minutes before the window — calibrate the work to fit), **grace** (the first 10 minutes of the window — bring the current work to a stable point), and **overdue** (past the grace — stop now).

**What it does not do, by design:** it is wired on `UserPromptSubmit` only — never `PreToolUse`, `Stop`, `SessionStart` or anything else that fires during a turn. A task started before the window continues to its real end; only the *next* prompt is warned about. At the window's end hour (exclusive), prompts go through silently again with no restart needed — the hook reads the wall clock on every prompt.

**Night pass (`#jedors`)** — for a goal meant to run while you sleep. Put `#jedors` (case-insensitive) anywhere in the prompt that launches it: the hook writes `%USERPROFILE%\.claude\curfew.nuit.json` and then stays silent for **every** prompt until the window's `end` hour — loop iterations, background agents waking up, a prompt you type at 3 a.m. The exemption is deliberately global rather than per-session, since the session that says `#jedors` is not necessarily the one relaunched during the night. The marker expires on its own; deleting the file hands control back to the curfew immediately. An absent, unreadable or undated marker means a normal curfew (fail-open, like everything else here).

**The hours are never committed.** The hook is generic; the schedule lives in a local file that `deploy.ps1` does not track, so it survives every `deploy.ps1 -Pull`:

```powershell
# %USERPROFILE%\.claude\curfew.local.json   (example values - use your own)
{
  "enabled": true,
  "start": "01:00",
  "end": "08:00"
}
```

| Key | Meaning |
| --- | --- |
| `enabled` | Optional, defaults to `true`. `false` makes the hook inert without deleting the file. |
| `start` | Start of the curfew, **inclusive**. `"HH:mm"` or a plain hour (`23`). |
| `end` | End of the curfew, **exclusive**. Same formats. `end < start` simply means the window crosses midnight. Also the hour a `#jedors` night pass expires. |
| `warnBefore` | Optional, defaults to `[15, 10, 5]`. Notice thresholds in minutes before `start`. Values outside `]0, 720]` are dropped; an empty or unreadable list falls back to the default. |
| `graceAfter` | Optional, defaults to `10`. Minutes of grace after `start` (integer in `[0, 60]`). `0` means a hard stop from the first minute. |
| `message` | Optional. Replaces the message shown to the user inside the window (grace **and** overdue). |

Wire it in `settings.json` (already done in this repo's `settings.json`, alongside `error-vault-trigger.ps1`):

```json
"UserPromptSubmit": [
  { "hooks": [
      { "type": "command", "command": "pwsh -NoProfile -ExecutionPolicy Bypass -File \"$USERPROFILE/.claude/hooks/curfew.ps1\"", "timeout": 10 }
  ] }
]
```

**Fail-open by construction**: no config file, unreadable JSON, unparsable hours, `start == end`, a broken night marker, or any internal error → exit 0, no warning, prompt untouched. A broken curfew must never lock you out. This is also why cloning this repo without a `curfew.local.json` changes nothing for you.

The hook reads the event payload on stdin (to spot `#jedors`), but **only when stdin is redirected** — run by hand in a terminal it returns at once instead of waiting on an input that will never come.

Tests: `pwsh -NoProfile -File claude-code/hooks/tests/curfew.tests.ps1` (95 assertions, no Pester needed) — boundary hours, midnight wrap, notice thresholds, grace, night pass arming/expiry/cleanup, fail-open paths, and a check that no `curfew` hook is ever registered on an event that would interrupt an active task.

## Vision delegation (no-vision backends) + Alt+V screenshot shortcut

`glm-5.2:cloud` gives Claude Code a 1M-token context window (~4x kimi) but **no image support**: calling `Read` on a `.png/.jpg/...` crashes the conversation with `API Error: 400 this model does not support image input`. The chain below lets a no-vision backend still "see" images by delegating them to `kimi-k2.7-code:cloud` (MoonViT vision) through the local Ollama API.

### How it fits together

1. **`kimi-vision.ps1`** — the delegator. Base64-encodes the image, POSTs `{model:"kimi-k2.7-code:cloud", stream:false, messages:[{role:"user", content:<prompt>, images:[<b64>]}]}` to `http://localhost:11434/api/chat`, and prints `resp.message.content` on stdout. The assistant calls it **instead of** `Read` whenever an image must be analyzed, then uses kimi's textual answer to respond. Transparent, no conversation crash.
2. **`screenshot-shortcut.ahk`** (AutoHotkey v2) — `Alt+V` saves a clipboard image to disk (or finds the most recent screenshot as a fallback) and pastes its **path** into the active window, including Claude Code in Warp. The assistant can then read that local path directly or feed it to `kimi-vision.ps1`. Sending a path as text avoids the native image upload and the no-vision backend 400 error.
3. **`keybindings.json`** — explicitly maps `meta+v` to `null`. On Windows, Claude Code interprets `meta` as Alt and otherwise reserves `Alt+V` for `chat:imagePaste`; unbinding it leaves the shortcut entirely to AutoHotkey.

### Install (automatic and idempotent)

```powershell
# The Windows bootstrap does this automatically. For a manual redeploy:
& ~/.claude/scripts/install-screenshot-shortcut.ps1
```

That script ensures AutoHotkey v2 is installed (`winget install AutoHotkey.AutoHotkey --scope user`, no UAC), creates the Startup `.lnk` so AHK relaunches at every boot, and launches the `.ahk` immediately so `Alt+V` is active without a reboot. Re-run it safely anytime (`#SingleInstance Force` in the `.ahk` prevents duplicates). `-Uninstall` removes the `.lnk` and stops AHK. Claude Code watches `keybindings.json`, so the native image-paste unbinding applies without restarting the session.

### Install-free fallback (no AutoHotkey)

If you don't want to install AutoHotkey, `latest-screenshot.ps1` does the same search with no dependency. Create a `.lnk` (Shortcut key `Ctrl+Alt+V` — Windows shortcuts require a modifier) targeting:

```
pwsh -NoProfile -File %USERPROFILE%\.claude\scripts\latest-screenshot.ps1 -Paste
```

`Ctrl+Alt+V` then copies the latest screenshot's path to the clipboard and pastes it. `Alt+V` (single modifier, no Ctrl) is only achievable with AutoHotkey — that's the reason for the install above.

### Important caveats

- **Bypass permissions + no system guardrail**: in `bypassPermissions` mode, Claude Code skips `PreToolUse` hooks by design (issues #6305/#41151/#53589), so there is **no hook that blocks `Read` on an image**. The delegation relies entirely on the assistant following its persistent memory (auto-memory file `image-vision-delegation-glm`) — no system-level safety net. A `PreToolUse` model-aware blocker (`block-image-reads.ps1`) was tried and removed for this reason; do not re-add one while running in bypass.
- **`Alt+V` always grabs the most recent screenshot.** For an older image, paste the path manually. Supported extensions in the `.ahk`: `png/jpg/jpeg/webp/bmp`. Other formats (`gif/svg/tiff/heic/avif/ico`) → paste the path manually; `kimi-vision.ps1` still handles them.
- **If direct text insertion is swallowed by the terminal**, the path is still on the clipboard: press `Ctrl+V` yourself.
- Ollama must be running (cloud sign-in OK) for `kimi-vision.ps1` to reach `kimi-k2.7-code:cloud`.

## Login / Resume shortcuts (Alt+L / Alt+R)

Useful when you juggle multiple Claude Pro accounts and switch often: `/login` (with the default login method auto-confirmed) and `/resume` get a dedicated shortcut wired into every place you might be typing — native Windows Terminal, VS Code's integrated terminal, and Termux on the phone (SSH into the desktop).

| Where | `Alt+L` (login) | `Alt+R` (resume) |
| --- | --- | --- |
| Windows Terminal | `sendInput` action, `/login\r\r` — the 2nd `\r` accepts the default login method | `sendInput`, `/resume\r` |
| VS Code integrated terminal | `sendSequence`, same two escape sequences, `when: terminalFocus` | same |
| Claude Code itself (`keybindings.json`) | `alt+l` → `command:login` — fallback for terminals `sendInput`/`sendSequence` can't reach | no `command:resume` equivalent — use a terminal-level shortcut instead |

`sendInput` acts client-side, so the Windows Terminal shortcut also fires inside an SSH session opened from WT (e.g. attached from the phone through the Zellij web tunnel).

- **Windows Terminal**: deployed automatically by `windows/install.ps1` (part of `wt-settings.json`).
- **VS Code**: not auto-deployed — `keybindings.json` on a real machine usually already holds unrelated personal bindings, so a blind overwrite would be destructive. Merge `windows/files/vscode-keybindings.json` into `%APPDATA%\Code\User\keybindings.json` by hand.
- **Termux**: two extra-keys macros (`login`, `res`) on the phone's keyboard row — see [`android/README.md`](../android/README.md#extra-keys-row).

## Sessionizer (`pwsh` / F2)

`windows-sessionizer/sessionizer.ps1` est le menu fzf natif Windows qui fusionne, en une liste : `●` **sessions** Zellij actives (attach), `○` **projets** sous `C:\dev` sans session (créer dans le dossier), `○` **vaults Obsidian** en violet (PowerShell natif), et **nouvelles** sessions ad-hoc (Ctrl+N). Depuis Termux, `pwsh` demande cette même vue `all` que F2 au PC, puis ouvre le choix via le tunnel Zellij web pour que la session reste joignable depuis le bureau.

> **Why Zellij, not tmux?** tmux has no native Windows build, and the only native-Windows tmux clone (psmux) was too buggy (laggy Claude TUI, Esc-blocked-after-Ctrl+letter, truecolor off). Zellij ships a native-Windows ConPTY binary (≥ 0.44) that runs `pwsh` natively (full-speed C: I/O) **with** detach/reattach. So Windows-native Zellij is now the daily mux. tmux stays installed as a dependency of agent orchestrators and a `0.x` fallback.

| Entry point | View | World — shows |
|---|---|---|
| `pwsh` / `pwshm` — phone | `all` | same unified scope as F2: native Windows sessions, projects and vaults |
| **F2** — desktop Windows shell | `all` (default) | native Windows sessions, projects and vaults |

`sleep-pc` suspends the desktop, `stop-pc` shuts it down — both run a native Windows command through the Windows sshd (port 2222) + `$WIN_USER`.

> **Picking anything from the phone opens native Windows PowerShell — automatically.** Termux lists the unified menu through the Windows sshd, then attaches the local Zellij web client through an SSH tunnel: `127.0.0.1:<local> -> desktop:127.0.0.1:8082`. A token lives only in `~/.config/pc-zellij-web-token` on the phone. The Windows account for the SSH tunnel can't be derived from the phone, so it's read from **`$WIN_USER`** — set it in `~/.bashrc.local` (`export WIN_USER=<your Windows account>`), kept out of git so the versioned `bashrc` stays shareable.

On Termux, the `pwsh` menu auto-refreshes while it is open, keeping the last good list if the PC is briefly unreachable. It starts on the first selectable row and skips decorative section titles for arrows and taps.

| Key | Action |
|---|---|
| ⏎ | open / attach |
| ↹ Tab | switch category (projects ⇄ vaults) — F2 `all` view only |
| Ctrl+N | new session (free name) |
| Ctrl+X | kill the active session (see below) |
| Ctrl+G | toggle the help header |
| Esc | cancel a Ctrl+N/R/X prompt typed by mistake |
| Click / tap | move the `▌` cursor to that line (visual feedback, does **not** open) |
| Double-click / double-tap | open the item under the pointer (titles stay inert) |

> **Mouse** works over `pc` (ssh) and F2 (desktop) only — `pcm` (mosh) doesn't route the mouse, so taps never reach fzf. There is **no hover highlight**: fzf tracks clicks and scroll (terminal mouse modes 1000/1002), not bare cursor motion (1003), so it gets no event until you actually click. Opening is on the **second** click *because* there's no hover — the first click moves the `▌` cursor onto the line (your "look before you leap"), the second confirms. **`◆` titles are never selectable by mouse**: clicking one bounces the cursor to its section's first item (fzf positions the cursor on the clicked line *before* the bound action — `terminal.go:7848` — so the bind can detect a title via `$FZF_POS` and step off it), and a double-click on a title is ignored.

### Ctrl+X: kill ≠ delete

`Ctrl+X` runs `zellij kill-session` — it ends the **running process** (the live shell / `claude`), never any file on disk. (Ctrl+R rename is disabled with Zellij: its CLI can't rename a detached session.) What happens to the **menu row** depends on whether a folder backs it:

| Session killed | Menu row after kill | Why |
|---|---|---|
| 🟢 `~/dev` **project** | ✅ stays, flips to `○` **inactive** | the project folder still exists to anchor the `○` row |
| 🟣 **Obsidian vault** | ✅ stays, flips to `○` **inactive** | same — the vault folder still exists |
| ⚪ **disposable** (free name, no folder) | ❌ **disappears entirely** | a `○` row means "folder exists, no session"; a disposable session has no folder, so nothing is left to list |

A "real" session is therefore **never lost** from the list — `Ctrl+X` just deactivates it (`●` → `○`), and you re-enter it later (`claude --resume` recovers the transcript). Only a truly disposable session vanishes, which is the whole point of "disposable". The table above is what happens; the prompt itself stays terse (see below) since `Ctrl+X` already means kill.

Confirming is a **single keypress**, not a typed `y`/`n` + Enter: `Ctrl+X` prints `Enter to kill name (Esc to cancel)`, press **Enter** to confirm, **Esc** (or any other key) to cancel. Same one-key reader on both entry points — `[Console]::ReadKey` in `sessionizer.ps1` (F2, desktop) and a raw `stty -icanon` read in `_pc_read_confirm` (`pwsh` from Termux) — so the gesture feels identical whether you're on the desktop or SSH'd in from the phone. The name itself is colored like its section — folder yellow for a `~/dev` project, violet for an Obsidian vault, plain for a disposable session — carried through a 5th TSV field (`project`/`vault`/`session`) that both `sessionizer.ps1` and `_pc_read_confirm`'s caller read off the selected row.

## Statusline — technical details

Private Claude Code endpoint for live usage:
```
GET https://api.anthropic.com/api/oauth/usage
Authorization: Bearer <accessToken from ~/.claude/.credentials.json>
anthropic-beta: oauth-2025-04-20
```

| Permission mode | Path color (RGB) |
| --- | --- |
| `bypassPermissions` | 255,121,198 (hot pink) |
| `plan` | 139,233,253 (light cyan) |
| `acceptEdits` | 80,250,123 (green) |
| `dontAsk` | 189,147,249 (purple) |
| `auto` | 255,184,108 (orange) |

| Usage % | Bar/text color |
| --- | --- |
| `< 50%` | green (80,250,123) |
| `50–80%` | yellow (241,250,140) |
| `> 80%` | red (255,85,85) |

Caches: usage 60s (`~/.claude/usage-cache.json`), auth 1h (`~/.claude/auth-status-cache.json`).

## Effort level rendering

The 5 effort levels (`low`, `medium`, `high`, `xhigh`, `max`) render as follows in the statusline:

| Level | Rendering |
| --- | --- |
| `low` | yellowBright bold |
| `medium` | greenBright bold |
| `high` | blueBright bold |
| `xhigh` | uniform `#F2C1E9` (rose pastel) bold |
| `max` | uniform `#FF79C6` (rose vif) bold — **exact same color as the path in `bypassPermissions` mode** |

All renderings are static. `xhigh` and `max` use truecolor RGB (`\e[38;2;R;G;Bm`) explicitly, so the color is identical on every Windows Terminal theme and unaffected by the Claude Code `dark-ansi` theme. Claude Code's internal `/effort` picker animates `xhigh` and `max` at ~10 Hz (Ink/React inside the binary), but the statusline does not animate — `claude.exe` invokes the statusline subprocess on state changes, not on a timer, so per-frame animation from a subprocess is impractical. Both `.ps1` and `.exe` renderings are visually equivalent.

## Build the Rust statusline (optional)

The default `.ps1` statusline works with no prerequisites and is visually equivalent to the Rust binary. Compile the Rust version only if you want the marginal speedup of a single-shot compiled binary over a PowerShell process.

### Prerequisites

1. **Smart App Control = Off** — otherwise Windows 11 blocks execution of any unsigned Rust binary (OS error 4551 on `cargo build` too). Settings → Privacy & security → Smart App Control. ⚠️ **Disabling SAC is permanent**: re-enabling requires a full Windows reset.
2. **Rust toolchain**:
   ```powershell
   winget install Rustlang.Rustup
   ```
   If Visual Studio Build Tools are absent, `build.ps1` automatically installs WinLibs/MinGW via winget (provides `gcc.exe` for ring/rustls) and builds with `stable-gnu`.

### Build

```powershell
cd C:\dev\dev-environment\claude-code\statusline-rs
.\build.ps1
```

`build.ps1` builds in `--release` (LTO + strip), redirects `CARGO_TARGET_DIR` to `%LOCALAPPDATA%\statusline-build\target` (trusted zone outside `C:\dev`, avoids SAC blocks on cargo build scripts), falls back to `stable-gnu` plus WinLibs/MinGW when MSVC is absent, then copies the binary to `~/.claude/statusline.exe`.

## Credit

The `/usage` endpoint was discovered by [Melvynx (codelynx.dev)](https://codelynx.dev/posts/claude-code-usage-limits-statusline) via Proxyman.
