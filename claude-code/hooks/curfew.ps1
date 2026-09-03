# curfew.ps1 -- hook UserPromptSubmit "couvre-feu" (avertissement, fail-open).
#
# Ne bloque RIEN. Trois regimes declenchent un avertissement, jamais un refus :
#   1. PREAVIS   : prompt soumis peu avant le debut de la fenetre. Trois paliers
#                  par defaut (15, 10 puis 5 minutes avant).
#   2. TOLERANCE : prompt soumis dans les premieres minutes de la fenetre
#                  (10 par defaut). L'heure n'est pas a la seconde pres : ce
#                  delai sert a amener le travail en cours a un point stable.
#   3. DEPASSE   : au-dela de la tolerance. Consigne d'arret immediat.
# Dans les trois cas l'avertissement est double :
#   - visible par l'utilisateur   (champ JSON "systemMessage") ;
#   - injecte dans le contexte de Claude (hookSpecificOutput.additionalContext),
#     avec la consigne de calibrer puis de conclure le tour au plus vite.
#
# LIMITE ASSUMEE : un hook UserPromptSubmit ne s'execute qu'au moment ou un
# prompt est soumis. Les preavis n'apparaissent donc pas spontanement a T-15 ;
# ils s'affichent sur le premier prompt envoye dans le palier. C'est aussi ce
# qui garantit qu'une tache deja en cours n'est jamais interrompue (le hook
# n'est branche que sur UserPromptSubmit, jamais PreToolUse / Stop /
# SessionStart).
#
# Configuration : fichier JSON LOCAL, jamais versionne (les horaires sont
# personnels et le repo est public). Chemin par defaut :
#
#   %USERPROFILE%\.claude\curfew.local.json
#   { "enabled": true, "start": "23:00", "end": "05:00",
#     "warnBefore": [15, 10, 5], "graceAfter": 10 }
#
#   enabled    : optionnel, defaut true. false -> hook inerte.
#   start      : debut de la fenetre, INCLUS  ("HH:mm" ou heure entiere).
#   end        : fin de la fenetre, EXCLUE    ("HH:mm" ou heure entiere).
#   warnBefore : optionnel, defaut [15, 10, 5]. Paliers de preavis en minutes
#                avant "start". Valeurs hors ]0, 720] ignorees ; liste vide ou
#                illisible -> retour au defaut.
#   graceAfter : optionnel, defaut 10. Minutes de tolerance apres "start"
#                (entier de [0, 60] ; illisible -> defaut). 0 = arret sec des
#                la premiere minute.
#   message    : optionnel, remplace le message affiche pendant la fenetre
#                (tolerance ET depassement).
#
# NUIT BLANCHE (#jedors) : un objectif lance le soir doit pouvoir tourner
# pendant que l'utilisateur dort. Un prompt qui porte le mot-cle "#jedors"
# arme une nuit blanche : le hook ecrit un marqueur
#
#   %USERPROFILE%\.claude\curfew.nuit.json
#   { "armedAt": "2026-09-03T22:50:00", "until": "2026-09-04T05:00:00" }
#
# et se tait pour TOUS les prompts jusqu'a "until" (la prochaine occurrence de
# "end"), quel qu'en soit le texte : iterations de boucle, relances d'agents,
# reveils programmes, prompt tape a 3 h. L'exemption est volontairement globale
# et non liee a une session : celle qui dit "#jedors" n'est pas forcement celle
# qui sera relancee dans la nuit. Le marqueur expire seul, rien a desarmer le
# lendemain ; le supprimer a la main rend la main au couvre-feu immediatement.
# Marqueur absent, illisible ou date invalide -> couvre-feu normal (fail-open).
#
# deploy.ps1 ne touche pas ce fichier (whitelist : statusline.ps1,
# settings.json, keybindings.json, hooks/, skills/, device-context, scripts,
# tools) : la config survit donc a tout -Pull. Sans fichier de config, le hook
# n'avertit de rien -- c'est ce qui le rend partageable tel quel.
#
# Contrat de sortie (doc hooks Claude Code, evenement UserPromptSubmit) :
#   exit 0 + stdout vide  -> prompt autorise, rien affiche ;
#   exit 0 + stdout JSON  -> prompt autorise, "systemMessage" montre a
#                            l'utilisateur et "additionalContext" injecte dans
#                            le contexte du modele.
# Le code 2 (blocage) n'est plus jamais emis, y compris en cas d'erreur interne :
# un avertissement casse ne doit jamais enfermer l'utilisateur dehors.
#
# ASCII-only (regle dev-environment) : pas de diacritiques dans ce fichier.

[CmdletBinding()]
param(
    # Heure simulee ("HH:mm" ou "yyyy-MM-dd HH:mm"). Reserve aux tests ;
    # le hook n'est jamais appele avec ce parametre par Claude Code.
    [string]$Now,
    # Chemin de config alternatif. Reserve aux tests.
    [string]$ConfigPath,
    # Chemin du marqueur de nuit blanche. Reserve aux tests.
    [string]$MarkerPath
)

$ErrorActionPreference = 'SilentlyContinue'

$DefaultWarnBefore  = @(15, 10, 5)
$DefaultGraceAfter  = 10
$NightKeyword       = '#jedors'
$MarkerFormat       = 'yyyy-MM-ddTHH:mm:ss'

# "HH:mm", "23" ou 23 -> minutes depuis minuit. $null si illisible.
function ConvertTo-MinuteOfDay($value) {
    if ($null -eq $value) { return $null }
    $text = ([string]$value).Trim()
    if ($text -match '^([0-9]{1,2})$') {
        $h = [int]$Matches[1]
        if ($h -gt 24) { return $null }
        return ($h % 24) * 60
    }
    if ($text -match '^([0-9]{1,2}):([0-9]{2})$') {
        $h = [int]$Matches[1]
        $m = [int]$Matches[2]
        if ($h -gt 24 -or $m -gt 59) { return $null }
        return (($h % 24) * 60 + $m)
    }
    return $null
}

function Format-Window($minuteOfDay) {
    return ('{0:00}:{1:00}' -f [math]::Floor($minuteOfDay / 60), ($minuteOfDay % 60))
}

# Paliers de preavis : entiers de ]0, 720], tries du plus large au plus proche.
# Toute config vide ou illisible retombe sur le defaut.
function Get-WarnBefore($value) {
    if ($null -eq $value) { return $DefaultWarnBefore }
    $clean = @()
    foreach ($item in @($value)) {
        $n = 0
        if ([int]::TryParse(([string]$item).Trim(), [ref]$n) -and $n -gt 0 -and $n -le 720) {
            $clean += $n
        }
    }
    if ($clean.Count -eq 0) { return $DefaultWarnBefore }
    return @($clean | Sort-Object -Unique -Descending)
}

# Tolerance apres le debut de la fenetre, en minutes. Entier de [0, 60] ;
# toute valeur illisible retombe sur le defaut.
function Get-Grace($value) {
    if ($null -eq $value) { return $DefaultGraceAfter }
    $n = 0
    if ([int]::TryParse(([string]$value).Trim(), [ref]$n) -and $n -ge 0 -and $n -le 60) {
        return $n
    }
    return $DefaultGraceAfter
}

function Write-CurfewPayload($userMsg, $modelMsg) {
    $payload = [ordered]@{
        systemMessage      = $userMsg
        hookSpecificOutput = [ordered]@{
            hookEventName     = 'UserPromptSubmit'
            additionalContext = $modelMsg
        }
    }
    [Console]::Out.WriteLine(($payload | ConvertTo-Json -Depth 4 -Compress))
}

# Texte du prompt soumis, lu dans le payload JSON de l'evenement.
# Le garde-fou compte autant que la lecture : stdin n'est consomme QUE s'il est
# redirige. Sur une entree restee sur la console (test lance a la main, appel
# direct), le hook rendrait la main seulement au bout du timeout.
function Get-PromptText {
    if (-not [Console]::IsInputRedirected) { return '' }
    try {
        $raw = [Console]::In.ReadToEnd()
        if ([string]::IsNullOrWhiteSpace($raw)) { return '' }
        return [string]($raw | ConvertFrom-Json).prompt
    } catch {
        return ''
    }
}

# Prochaine occurrence d'une heure de la journee, a partir de $from (exclu).
function Get-NextOccurrence([datetime]$from, [int]$minuteOfDay) {
    $candidate = $from.Date.AddMinutes($minuteOfDay)
    if ($candidate -le $from) { $candidate = $candidate.AddDays(1) }
    return $candidate
}

# Date de fin de la nuit blanche armee, ou $null si le marqueur est absent,
# illisible, ou porte une date qu'on ne sait pas lire.
function Get-NightPassUntil($path) {
    if (-not (Test-Path $path -PathType Leaf)) { return $null }
    try {
        $marker = Get-Content $path -Raw | ConvertFrom-Json
    } catch {
        return $null
    }
    if ($null -eq $marker) { return $null }
    # ConvertFrom-Json (PowerShell 7) convertit d'office une date ISO en
    # [datetime] : le champ n'est donc pas toujours une chaine, et le forcer en
    # chaine donnerait le format de la machine ("09/04/2026 05:00:00"), que
    # TryParseExact refuserait.
    if ($marker.until -is [datetime]) { return [datetime]$marker.until }
    $until = [datetime]::MinValue
    $ok = [datetime]::TryParseExact([string]$marker.until, $MarkerFormat,
        [Globalization.CultureInfo]::InvariantCulture,
        [Globalization.DateTimeStyles]::None, [ref]$until)
    if (-not $ok) { return $null }
    return $until
}

try {
    if ($ConfigPath) {
        $cfgPath = $ConfigPath
    } else {
        $cfgPath = Join-Path $env:USERPROFILE '.claude\curfew.local.json'
    }
    if (-not (Test-Path $cfgPath -PathType Leaf)) { exit 0 }

    $cfg = Get-Content $cfgPath -Raw | ConvertFrom-Json
    if ($null -eq $cfg) { exit 0 }
    if ($null -ne $cfg.enabled -and -not [bool]$cfg.enabled) { exit 0 }

    $start = ConvertTo-MinuteOfDay $cfg.start
    $end   = ConvertTo-MinuteOfDay $cfg.end
    if ($null -eq $start -or $null -eq $end) { exit 0 }
    if ($start -eq $end) { exit 0 }   # fenetre vide : rien a signaler

    if ($Now) {
        # Formats personnalises uniquement : un specificateur standard d'un
        # seul caractere ('o', 's'...) dans le tableau fait echouer TOUT
        # l'appel a TryParseExact, y compris les formats valides.
        $formats = [string[]]@('HH:mm', 'yyyy-MM-dd HH:mm', 'yyyy-MM-ddTHH:mm')
        $parsed = [datetime]::MinValue
        $ok = [datetime]::TryParseExact($Now, $formats,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::None, [ref]$parsed)
        if (-not $ok) { exit 0 }
        $nowDt = $parsed
    } else {
        $nowDt = Get-Date
    }
    $current = $nowDt.Hour * 60 + $nowDt.Minute

    # --- Nuit blanche -------------------------------------------------------
    # Deux temps : le prompt qui porte "#jedors" arme le marqueur, et tant que
    # ce marqueur vaut, plus rien n'est dit -- a n'importe quelle heure et quel
    # que soit le prompt, puisque les relances de la nuit ne repetent pas
    # forcement le mot-cle.
    if ($MarkerPath) {
        $markerFile = $MarkerPath
    } else {
        $markerFile = Join-Path $env:USERPROFILE '.claude\curfew.nuit.json'
    }

    if ((Get-PromptText) -match [regex]::Escape($NightKeyword)) {
        $until = Get-NextOccurrence $nowDt $end
        $marker = [ordered]@{
            armedAt = $nowDt.ToString($MarkerFormat)
            until   = $until.ToString($MarkerFormat)
        }
        Set-Content -Path $markerFile -Encoding ascii -Value ($marker | ConvertTo-Json -Depth 3)

        $userMsg = "Nuit blanche armee : plus un mot du couvre-feu ({0}) jusqu'a {1}. Claude peut mener l'objectif a son terme pendant que tu dors." `
                       -f ("{0} - {1}" -f (Format-Window $start), (Format-Window $end)), (Format-Window $end)
        $modelMsg = ("NUIT BLANCHE ARMEE ({0}) -- l'utilisateur part dormir et te laisse la nuit pour travailler, jusqu'a {1}. " +
                     "Aucune contrainte d'horaire jusque-la : ne calibre pas court, n'annonce aucun arret, mene l'objectif a son terme. " +
                     "Les relances de cette nuit (boucle, agent, reveil programme) resteront silencieuses elles aussi ; " +
                     "a {1} la limite de nuit reprend d'elle-meme.") `
                        -f $NightKeyword, (Format-Window $end)
        Write-CurfewPayload $userMsg $modelMsg
        exit 0
    }

    $nightUntil = Get-NightPassUntil $markerFile
    if (Test-Path $markerFile -PathType Leaf) {
        if ($null -ne $nightUntil -and $nowDt -lt $nightUntil) { exit 0 }
        # Nuit finie, ou marqueur qu'on ne sait pas lire : on rend la main au
        # couvre-feu et on nettoie, pour ne pas laisser un fichier mort armer
        # un doute la nuit suivante.
        Remove-Item $markerFile -Force -ErrorAction SilentlyContinue
    }
    # ------------------------------------------------------------------------

    $startText = Format-Window $start
    $endText   = Format-Window $end
    $nowText   = Format-Window $current
    $window    = "{0} - {1}" -f $startText, $endText

    # Fenetre normale (start < end) ou a cheval sur minuit (start > end).
    if ($start -lt $end) {
        $inWindow = ($current -ge $start -and $current -lt $end)
    } else {
        $inWindow = ($current -ge $start -or $current -lt $end)
    }

    if ($inWindow) {
        # Tolerance humaine : les premieres minutes de la fenetre servent a
        # finir proprement le point en cours. Au-dela, on est en retard.
        $grace = Get-Grace $cfg.graceAfter
        $minutesIn = $current - $start
        if ($minutesIn -lt 0) { $minutesIn += 1440 }
        $deadline     = Format-Window (($start + $grace) % 1440)
        $inGrace      = ($minutesIn -lt $grace)
        $graceLeft    = $grace - $minutesIn

        if ($cfg.message) {
            $userMsg = [string]$cfg.message
        } elseif ($inGrace) {
            $userMsg = "Couvre-feu depuis {0} : il est {1}. Tolerance de {2} min pour boucler -- limite dure a {3} ({4} min). Claude finit le point en cours, sans rien entamer de nouveau." `
                           -f $startText, $nowText, $grace, $deadline, $graceLeft
        } else {
            $userMsg = "Couvre-feu depasse : il est {0}, soit {1} min apres {2} (tolerance {3} min epuisee). Claude s'arrete maintenant. Reprise a {4}." `
                           -f $nowText, $minutesIn, $startText, $grace, $endText
        }

        if ($inGrace) {
            $modelMsg = ("COUVRE-FEU EN COURS (tolerance) -- il est {0}, le couvre-feu {1} a commence a {2}. " +
                         "Rien n'est bloque et l'utilisateur n'exige pas l'arret a la seconde pres : tu disposes d'une tolerance " +
                         "de {3} minutes, soit jusqu'a {4} au plus tard ({5} minutes restantes). " +
                         "Sers-t'en pour amener le travail en cours a un point stable, puis arrete-toi. " +
                         "N'entame rien qui ne tienne pas dans ce delai ; si la demande est plus longue, dis-le en une phrase " +
                         "et propose de la reprendre apres {6}.") `
                            -f $nowText, $window, $startText, $grace, $deadline, $graceLeft, $endText
        } else {
            $modelMsg = ("COUVRE-FEU DEPASSE -- il est {0}, soit {1} minutes apres le debut du couvre-feu {2} ; " +
                         "la tolerance de {3} minutes est epuisee. Rien n'est bloque techniquement, mais l'utilisateur " +
                         "veut que tu t'arretes MAINTENANT : reponds en une ou deux phrases, n'ouvre aucun fichier, " +
                         "ne lance aucune commande, n'entame aucune tache. Resume l'etat du travail et propose de reprendre apres {4}.") `
                            -f $nowText, $minutesIn, $window, $grace, $endText
        }

        Write-CurfewPayload $userMsg $modelMsg
        exit 0
    }

    # Hors fenetre : reste-t-on dans un palier de preavis ?
    $minutesLeft = $start - $current
    if ($minutesLeft -lt 0) { $minutesLeft += 1440 }

    $warnBefore = Get-WarnBefore $cfg.warnBefore
    $threshold  = $null
    foreach ($w in $warnBefore) {
        if ($minutesLeft -le $w) { $threshold = $w }   # tri decroissant : le dernier retenu est le plus proche
    }
    if ($null -eq $threshold -or $minutesLeft -le 0) { exit 0 }

    $rank  = ([array]::IndexOf([int[]]$warnBefore, [int]$threshold)) + 1
    $total = $warnBefore.Count
    $last  = ($rank -eq $total)

    if ($last) {
        $userMsg = "Dernier rappel ({0}/{1}) : couvre-feu dans {2} min (a {3}). Ne lance plus rien de long ; Claude va conclure au plus court." `
                       -f $rank, $total, $minutesLeft, $startText
    } else {
        $userMsg = "Preavis couvre-feu ({0}/{1}) : il reste {2} min avant {3}. Claude va calibrer le travail pour tenir dans ce delai." `
                       -f $rank, $total, $minutesLeft, $startText
    }

    $modelMsg = ("PREAVIS COUVRE-FEU {0}/{1} -- il est {2}, le couvre-feu ({3}) commence dans {4} minutes. " +
                 "Rien n'est bloque, mais calibre le travail pour qu'il tienne dans ces {4} minutes : " +
                 "n'entame aucune tache que tu ne peux pas terminer avant {5}, et arrive a un point stable avant cette heure. " +
                 "Si la demande ne tient pas dans le delai, dis-le en une phrase et propose de la decouper ou de la reprendre apres {6}.") `
                    -f $rank, $total, $nowText, $window, $minutesLeft, $startText, $endText

    Write-CurfewPayload $userMsg $modelMsg
    exit 0
} catch {
    exit 0
}
