# curfew.tests.ps1 -- tests du hook UserPromptSubmit "couvre-feu".
#
# Sans dependance (pas de Pester : la machine n'a que Pester 3.x). Chaque cas
# lance le hook dans un process pwsh separe et verifie le code de sortie, la
# sortie standard et la sortie d'erreur.
#
#   pwsh -NoProfile -File claude-code/hooks/tests/curfew.tests.ps1
#
# Contrat du hook (evenement UserPromptSubmit de Claude Code) : il n'emet plus
# jamais le code 2 (blocage). Il sort toujours 0 ; quand il y a lieu d'avertir
# il ecrit sur stdout un JSON { systemMessage, hookSpecificOutput } -> message
# visible par l'utilisateur ET contexte injecte dans le modele. Deux cas
# d'avertissement : preavis (paliers avant "start") et fenetre elle-meme.
#
# ASCII-only (regle dev-environment) : pas de diacritiques dans ce fichier.

$ErrorActionPreference = 'Stop'

$TestsDir  = Split-Path -Parent $PSCommandPath
$HooksDir  = Split-Path -Parent $TestsDir
$RepoRoot  = Split-Path -Parent (Split-Path -Parent $HooksDir)
$HookPath  = Join-Path $HooksDir 'curfew.ps1'
$TmpDir    = Join-Path ([IO.Path]::GetTempPath()) ("curfew-tests-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Force $TmpDir | Out-Null

$script:Pass = 0
$script:Fail = 0

function Write-Result($ok, $label, $detail) {
    if ($ok) {
        $script:Pass++
        Write-Host ("  PASS  " + $label) -ForegroundColor Green
    } else {
        $script:Fail++
        Write-Host ("  FAIL  " + $label) -ForegroundColor Red
        if ($detail) { Write-Host ("        " + $detail) -ForegroundColor DarkGray }
    }
}

# Lance le hook et renvoie @{ Code; Out; Err; Json }.
#
# stdin est TOUJOURS redirige depuis un fichier : le hook ne lit le payload que
# si l'entree est redirigee, et un test qui laisserait stdin sur la console ne
# mesurerait donc pas le meme chemin qu'en production.
function Invoke-Hook {
    param([string]$Now, [string]$ConfigPath, [string]$MarkerPath, [string]$Prompt)

    $outFile = Join-Path $TmpDir ([guid]::NewGuid().ToString('N') + '.out')
    $errFile = Join-Path $TmpDir ([guid]::NewGuid().ToString('N') + '.err')
    $inFile  = Join-Path $TmpDir ([guid]::NewGuid().ToString('N') + '.in')
    # Start-Process joint les arguments par des espaces sans rien citer : une
    # heure datee ("2026-09-03 22:50") ou un chemin a espaces arriverait coupe
    # en deux, et pwsh la lirait comme un parametre positionnel de trop.
    $q = { param($v) '"' + $v + '"' }
    $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (& $q $HookPath))
    if ($Now)        { $argList += @('-Now', (& $q $Now)) }
    if ($ConfigPath) { $argList += @('-ConfigPath', (& $q $ConfigPath)) }
    # Sans marqueur explicite, un chemin neuf et absent : aucun test ne doit
    # dependre de la nuit blanche eventuellement armee sur la vraie machine.
    if (-not $MarkerPath) {
        $MarkerPath = Join-Path $TmpDir ([guid]::NewGuid().ToString('N') + '.marker.json')
    }
    $argList += @('-MarkerPath', (& $q $MarkerPath))

    # Payload UserPromptSubmit reel : { session_id, prompt, ... }. Sans prompt,
    # un fichier vide -- c'est le cas d'un payload absent ou illisible.
    $payload = ''
    if ($Prompt) {
        $payload = (@{ hook_event_name = 'UserPromptSubmit'; prompt = $Prompt } | ConvertTo-Json -Compress)
    }
    Set-Content -Path $inFile -Value $payload -Encoding ascii -NoNewline

    $p = Start-Process -FilePath 'pwsh' -ArgumentList $argList -NoNewWindow -Wait -PassThru `
                       -RedirectStandardInput $inFile `
                       -RedirectStandardOutput $outFile -RedirectStandardError $errFile
    $out = (Get-Content $outFile -Raw -ErrorAction SilentlyContinue)
    $json = $null
    if (-not [string]::IsNullOrWhiteSpace($out)) {
        try { $json = $out | ConvertFrom-Json } catch { $json = $null }
    }
    return @{
        Code = $p.ExitCode
        Out  = $out
        Err  = (Get-Content $errFile -Raw -ErrorAction SilentlyContinue)
        Json = $json
    }
}

function New-Config($name, $content) {
    $path = Join-Path $TmpDir $name
    Set-Content -Path $path -Value $content -Encoding ascii
    return $path
}

# Un cas = "le prompt passe toujours (exit 0), avec ou sans avertissement".
# $expectWarn : $true  -> stdout doit porter le JSON d'avertissement ;
#               $false -> stdout doit rester vide.
function Test-Warning($label, $expectWarn, $now, $configPath, $markerPath, $prompt) {
    $r = Invoke-Hook -Now $now -ConfigPath $configPath -MarkerPath $markerPath -Prompt $prompt
    $warned = -not [string]::IsNullOrWhiteSpace($r.Out)
    $detail = "exit=$($r.Code) ; averti=$warned (attendu $expectWarn) ; stdout=$($r.Out) ; stderr=$($r.Err)"
    Write-Result (($r.Code -eq 0) -and ($warned -eq $expectWarn)) $label $detail
    return $r
}

Write-Host ''
Write-Host 'Fenetre 23:00 -> 05:00 : avertissement, jamais de blocage' -ForegroundColor Cyan

$cfg = New-Config 'curfew.json' '{ "enabled": true, "start": "23:00", "end": "05:00" }'

Test-Warning '22:00 (hors preavis) -> aucun avertissement'   $false '22:00' $cfg | Out-Null
Test-Warning '23:00 -> avertissement de fenetre'             $true  '23:00' $cfg | Out-Null
Test-Warning '04:59 -> avertissement de fenetre'             $true  '04:59' $cfg | Out-Null
Test-Warning '05:00 -> aucun avertissement'                  $false '05:00' $cfg | Out-Null
Test-Warning '02:30 (coeur de nuit) -> avertissement'        $true  '02:30' $cfg | Out-Null
Test-Warning '12:00 (plein jour) -> aucun avertissement'     $false '12:00' $cfg | Out-Null

Write-Host ''
Write-Host "Contenu de l'avertissement de fenetre" -ForegroundColor Cyan

$r = Invoke-Hook -Now '23:30' -ConfigPath $cfg
Write-Result ($r.Code -eq 0) `
    'le prompt n est jamais bloque pendant la fenetre' "exit=$($r.Code)"
Write-Result ($null -ne $r.Json) `
    'stdout est un JSON exploitable par Claude Code' "stdout=$($r.Out)"
Write-Result ($r.Json.systemMessage -match '23:00' -and $r.Json.systemMessage -match '05:00') `
    "systemMessage (vu par l'utilisateur) nomme la fenetre" "systemMessage=$($r.Json.systemMessage)"
Write-Result ($r.Json.hookSpecificOutput.hookEventName -eq 'UserPromptSubmit') `
    'hookSpecificOutput.hookEventName = UserPromptSubmit' "hookEventName=$($r.Json.hookSpecificOutput.hookEventName)"

$ctx = [string]$r.Json.hookSpecificOutput.additionalContext
Write-Result ($ctx -match '23:00' -and $ctx -match '05:00') `
    'additionalContext (vu par Claude) nomme la fenetre' "additionalContext=$ctx"
Write-Result ($ctx -match "t'arretes MAINTENANT") `
    "additionalContext demande a Claude de s'arreter (tolerance epuisee a 23:30)" "additionalContext=$ctx"
Write-Result ([string]::IsNullOrWhiteSpace($r.Err)) `
    'rien sur stderr : plus aucun message de blocage' "stderr=$($r.Err)"

$custom = New-Config 'custom-message.json' '{ "enabled": true, "start": "23:00", "end": "05:00", "message": "Il est tard, coupe court." }'
$rc = Invoke-Hook -Now '23:30' -ConfigPath $custom
Write-Result ($rc.Json.systemMessage -eq 'Il est tard, coupe court.') `
    'le champ message de la config remplace le systemMessage' "systemMessage=$($rc.Json.systemMessage)"
Write-Result ($rc.Json.hookSpecificOutput.additionalContext -match 'COUVRE-FEU DEPASSE') `
    'un message personnalise ne desarme pas la consigne envoyee a Claude' "additionalContext=$($rc.Json.hookSpecificOutput.additionalContext)"

Write-Host ''
Write-Host 'Tolerance apres le debut : pas 23:00 pile, mais 10 min max (defaut)' -ForegroundColor Cyan

$g0 = (Invoke-Hook -Now '23:00' -ConfigPath $cfg).Json
$g5 = (Invoke-Hook -Now '23:05' -ConfigPath $cfg).Json
$g9 = (Invoke-Hook -Now '23:09' -ConfigPath $cfg).Json
$gx = (Invoke-Hook -Now '23:10' -ConfigPath $cfg).Json
$gy = (Invoke-Hook -Now '23:40' -ConfigPath $cfg).Json

Write-Result ($g0.systemMessage -match 'Tolerance de 10 min' -and $g0.systemMessage -match '23:10') `
    '23:00 : tolerance annoncee avec sa limite dure 23:10' "systemMessage=$($g0.systemMessage)"
Write-Result ($g5.systemMessage -match '5 min\)') `
    '23:05 : le message annonce les 5 minutes de tolerance restantes' "systemMessage=$($g5.systemMessage)"
Write-Result ($g9.hookSpecificOutput.additionalContext -match 'COUVRE-FEU EN COURS \(tolerance\)') `
    '23:09 : Claude recoit encore le regime tolerance' "additionalContext=$($g9.hookSpecificOutput.additionalContext)"
Write-Result ($gx.systemMessage -match 'depasse' -and $gx.systemMessage -match '10 min apres 23:00') `
    '23:10 : bascule en depassement (tolerance epuisee)' "systemMessage=$($gx.systemMessage)"
Write-Result ($gy.hookSpecificOutput.additionalContext -match 'COUVRE-FEU DEPASSE' -and
              $gy.hookSpecificOutput.additionalContext -match 'MAINTENANT') `
    '23:40 : Claude recoit la consigne d arret immediat' "additionalContext=$($gy.hookSpecificOutput.additionalContext)"

$grace0 = New-Config 'grace-zero.json' '{ "enabled": true, "start": "23:00", "end": "05:00", "graceAfter": 0 }'
$z = (Invoke-Hook -Now '23:00' -ConfigPath $grace0).Json
Write-Result ($z.hookSpecificOutput.additionalContext -match 'COUVRE-FEU DEPASSE') `
    'graceAfter=0 : arret sec des 23:00' "additionalContext=$($z.hookSpecificOutput.additionalContext)"

$grace5 = New-Config 'grace-five.json' '{ "enabled": true, "start": "23:00", "end": "05:00", "graceAfter": 5 }'
$f4 = (Invoke-Hook -Now '23:04' -ConfigPath $grace5).Json
$f5 = (Invoke-Hook -Now '23:05' -ConfigPath $grace5).Json
Write-Result ($f4.systemMessage -match 'Tolerance de 5 min' -and $f4.systemMessage -match '23:05') `
    'graceAfter=5 : 23:04 encore dans la tolerance' "systemMessage=$($f4.systemMessage)"
Write-Result ($f5.systemMessage -match 'depasse') `
    'graceAfter=5 : 23:05 deja en depassement' "systemMessage=$($f5.systemMessage)"

$graceBad = New-Config 'grace-bad.json' '{ "enabled": true, "start": "23:00", "end": "05:00", "graceAfter": "banane" }'
$gb = (Invoke-Hook -Now '23:05' -ConfigPath $graceBad).Json
Write-Result ($gb.systemMessage -match 'Tolerance de 10 min') `
    'graceAfter illisible -> retour au defaut 10 min' "systemMessage=$($gb.systemMessage)"

Write-Host ''
Write-Host 'Preavis : 3 paliers avant le debut de la fenetre (defaut 15 / 10 / 5)' -ForegroundColor Cyan

Test-Warning '22:44 (16 min avant) -> pas encore de preavis' $false '22:44' $cfg | Out-Null
Test-Warning '22:45 (15 min avant) -> preavis'               $true  '22:45' $cfg | Out-Null
Test-Warning '22:52 (8 min avant)  -> preavis'               $true  '22:52' $cfg | Out-Null
Test-Warning '22:59 (1 min avant)  -> preavis'               $true  '22:59' $cfg | Out-Null

$p1 = (Invoke-Hook -Now '22:45' -ConfigPath $cfg).Json
$p2 = (Invoke-Hook -Now '22:52' -ConfigPath $cfg).Json
$p3 = (Invoke-Hook -Now '22:57' -ConfigPath $cfg).Json

Write-Result ($p1.systemMessage -match '1/3') 'palier 1/3 a 15 min' "systemMessage=$($p1.systemMessage)"
Write-Result ($p2.systemMessage -match '2/3') 'palier 2/3 a 8 min'  "systemMessage=$($p2.systemMessage)"
Write-Result ($p3.systemMessage -match '3/3') 'palier 3/3 a 3 min'  "systemMessage=$($p3.systemMessage)"

Write-Result ($p1.systemMessage -match '15 min') `
    'le preavis annonce les minutes restantes reelles' "systemMessage=$($p1.systemMessage)"
Write-Result ($p3.systemMessage -match 'Dernier rappel') `
    'le troisieme palier est annonce comme dernier rappel' "systemMessage=$($p3.systemMessage)"
Write-Result ($p1.hookSpecificOutput.additionalContext -match 'PREAVIS COUVRE-FEU 1/3' -and
              $p1.hookSpecificOutput.additionalContext -match '23:00') `
    'additionalContext du preavis nomme le palier et l heure de debut' "additionalContext=$($p1.hookSpecificOutput.additionalContext)"

$warn2 = New-Config 'warn-two.json' '{ "enabled": true, "start": "13:00", "end": "14:00", "warnBefore": [30, 10] }'
Test-Warning 'warnBefore personnalise : 12:25 (35 min) -> aucun' $false '12:25' $warn2 | Out-Null
$w1 = (Invoke-Hook -Now '12:35' -ConfigPath $warn2).Json
$w2 = (Invoke-Hook -Now '12:55' -ConfigPath $warn2).Json
Write-Result ($w1.systemMessage -match '1/2') 'warnBefore [30,10] : 12:35 -> palier 1/2' "systemMessage=$($w1.systemMessage)"
Write-Result ($w2.systemMessage -match '2/2' -and $w2.systemMessage -match 'Dernier rappel') `
    'warnBefore [30,10] : 12:55 -> palier 2/2, dernier rappel' "systemMessage=$($w2.systemMessage)"

$warnBad = New-Config 'warn-bad.json' '{ "enabled": true, "start": "23:00", "end": "05:00", "warnBefore": ["banane", 0, 9999] }'
$wb = (Invoke-Hook -Now '22:50' -ConfigPath $warnBad).Json
Write-Result ($wb.systemMessage -match '/3') `
    'warnBefore illisible -> retour au defaut 15 / 10 / 5' "systemMessage=$($wb.systemMessage)"

Write-Host ''
Write-Host 'Fail-open : jamais de blocage, jamais de faux avertissement' -ForegroundColor Cyan

$missing = Join-Path $TmpDir 'absent.json'
Test-Warning 'config absente -> aucun avertissement' $false '23:30' $missing | Out-Null

$disabled = New-Config 'disabled.json' '{ "enabled": false, "start": "23:00", "end": "05:00" }'
Test-Warning 'enabled=false -> aucun avertissement' $false '23:30' $disabled | Out-Null
Test-Warning 'enabled=false -> aucun preavis non plus' $false '22:50' $disabled | Out-Null

$broken = New-Config 'broken.json' '{ ceci nest pas du json'
Test-Warning 'config illisible -> aucun avertissement' $false '23:30' $broken | Out-Null

$badHours = New-Config 'bad-hours.json' '{ "enabled": true, "start": "99:99", "end": "banane" }'
Test-Warning 'heures invalides -> aucun avertissement' $false '23:30' $badHours | Out-Null

$empty = New-Config 'empty-window.json' '{ "enabled": true, "start": "05:00", "end": "05:00" }'
Test-Warning 'fenetre vide (start = end) -> aucun avertissement' $false '05:00' $empty | Out-Null

Write-Host ''
Write-Host 'Hook generique : fenetre sans passage de minuit, heures entieres' -ForegroundColor Cyan

$day = New-Config 'daytime.json' '{ "enabled": true, "start": "13:00", "end": "14:00" }'
Test-Warning '13:30 dans une fenetre 13:00-14:00 -> avertissement' $true  '13:30' $day | Out-Null
Test-Warning '12:40 hors fenetre et hors preavis -> aucun'         $false '12:40' $day | Out-Null
Test-Warning '14:00 hors fenetre 13:00-14:00 -> aucun'             $false '14:00' $day | Out-Null

$ints = New-Config 'int-hours.json' '{ "enabled": true, "start": 23, "end": 5 }'
Test-Warning 'heures en entier (23 / 5) : 23:00 -> avertissement' $true  '23:00' $ints | Out-Null
Test-Warning 'heures en entier (23 / 5) : 22:00 -> aucun'         $false '22:00' $ints | Out-Null

Write-Host ''
Write-Host 'Nuit blanche : #jedors tait le couvre-feu jusqu a la fin de la fenetre' -ForegroundColor Cyan

function New-MarkerPath($name) {
    return (Join-Path $TmpDir ($name + '-' + [guid]::NewGuid().ToString('N').Substring(0, 6) + '.json'))
}

# Le marqueur est verifie sur son TEXTE : ConvertFrom-Json convertirait la date
# ISO en [datetime] et masquerait le format reellement ecrit sur le disque.
function Read-MarkerText($path) {
    if (-not (Test-Path $path -PathType Leaf)) { return '' }
    return [string](Get-Content $path -Raw)
}

# Armement pendant un palier de preavis : le preavis ne doit plus sortir.
$mArm = New-MarkerPath 'arm-preavis'
$a1 = Invoke-Hook -Now '2026-09-03 22:50' -ConfigPath $cfg -MarkerPath $mArm -Prompt 'Refais tout le CSS #jedors'
Write-Result ($a1.Code -eq 0) 'armement : le prompt passe (exit 0)' "exit=$($a1.Code) ; stderr=$($a1.Err)"
Write-Result ($a1.Json.systemMessage -match 'Nuit blanche' -and $a1.Json.systemMessage -match '05:00') `
    'armement : le message visible annonce la nuit blanche et son terme' "systemMessage=$($a1.Json.systemMessage)"
Write-Result ($a1.Json.systemMessage -notmatch 'Preavis' -and $a1.Json.systemMessage -notmatch 'Dernier rappel') `
    'armement a 22:50 : plus aucun preavis' "systemMessage=$($a1.Json.systemMessage)"
$actx = [string]$a1.Json.hookSpecificOutput.additionalContext
Write-Result ($actx -match 'NUIT BLANCHE') `
    'armement : Claude est prevenu que la nuit est libre' "additionalContext=$actx"
Write-Result ($actx -notmatch 'COUVRE-FEU' -and $actx -notmatch 'PREAVIS') `
    'armement : aucune consigne d arret ni de calibrage court' "additionalContext=$actx"

$mk = Read-MarkerText $mArm
Write-Result (-not [string]::IsNullOrWhiteSpace($mk)) 'armement : le marqueur de nuit blanche est ecrit' "marqueur=$mArm"
Write-Result ($mk -match '"until"\s*:\s*"2026-09-04T05:00:00"') `
    'armement a 22:50 : le marqueur expire au 05:00 suivant' "marqueur=$mk"
Write-Result ($mk -match '"armedAt"\s*:\s*"2026-09-03T22:50:00"') `
    "armement : le marqueur garde l'heure d'armement" "marqueur=$mk"

# Arme en pleine nuit : le terme reste le 05:00 du matin qui vient, pas J+1.
$mNight = New-MarkerPath 'arm-nuit'
Invoke-Hook -Now '2026-09-04 02:00' -ConfigPath $cfg -MarkerPath $mNight -Prompt '#jedors continue' | Out-Null
$mkn = Read-MarkerText $mNight
Write-Result ($mkn -match '"until"\s*:\s*"2026-09-04T05:00:00"') `
    'armement a 02:00 : expire a 05:00 le matin meme' "marqueur=$mkn"

# Casse et position du mot-cle.
$mCase = New-MarkerPath 'arm-casse'
$c1 = Invoke-Hook -Now '2026-09-03 23:30' -ConfigPath $cfg -MarkerPath $mCase -Prompt 'Lance le batch. #JeDors'
Write-Result ($c1.Json.systemMessage -match 'Nuit blanche') `
    '#JeDors : la casse du mot-cle est indifferente' "systemMessage=$($c1.Json.systemMessage)"

$mNoHash = New-MarkerPath 'arm-sans-croisillon'
$c2 = Invoke-Hook -Now '2026-09-03 23:30' -ConfigPath $cfg -MarkerPath $mNoHash -Prompt 'je dors mal en ce moment, corrige ce bug'
Write-Result ($c2.Json.hookSpecificOutput.additionalContext -match 'COUVRE-FEU') `
    'sans le croisillon, rien n est arme : le couvre-feu tient' "additionalContext=$($c2.Json.hookSpecificOutput.additionalContext)"
Write-Result (-not (Test-Path $mNoHash)) `
    'sans le croisillon, aucun marqueur n est ecrit' "marqueur=$mNoHash"

# Une fois arme, le silence vaut pour TOUT prompt, quel qu en soit le texte.
$mArmed = New-MarkerPath 'armed'
Set-Content -Path $mArmed -Encoding ascii -Value '{ "armedAt": "2026-09-03T22:50:00", "until": "2026-09-04T05:00:00" }'

Test-Warning 'arme : 02:00, prompt ordinaire -> aucun avertissement'  $false '2026-09-04 02:00' $cfg $mArmed 'continue la boucle' | Out-Null
Test-Warning 'arme : 23:30 -> aucun avertissement de fenetre'         $false '2026-09-03 23:30' $cfg $mArmed 'suite'            | Out-Null
Test-Warning 'arme : 22:50 -> aucun preavis non plus'                 $false '2026-09-03 22:50' $cfg $mArmed 'suite'            | Out-Null
Test-Warning 'arme : payload vide (relance sans prompt) -> silence'   $false '2026-09-04 03:00' $cfg $mArmed $null              | Out-Null
Write-Result (Test-Path $mArmed) 'arme : le marqueur survit aux prompts de la nuit' "marqueur=$mArmed"

# Expiration : passe 05:00, le couvre-feu revient de lui-meme.
$mExpired = New-MarkerPath 'expire'
Set-Content -Path $mExpired -Encoding ascii -Value '{ "armedAt": "2026-09-02T22:50:00", "until": "2026-09-03T05:00:00" }'
Test-Warning 'marqueur expire : le couvre-feu reprend' $true '2026-09-03 23:30' $cfg $mExpired 'un prompt du soir suivant' | Out-Null
Write-Result (-not (Test-Path $mExpired)) `
    'marqueur expire : le fichier est nettoye' "marqueur=$mExpired"

# Fail-open : un marqueur casse ne desarme pas le couvre-feu.
$mBroken = New-MarkerPath 'casse'
Set-Content -Path $mBroken -Encoding ascii -Value '{ ceci nest pas du json'
Test-Warning 'marqueur illisible -> couvre-feu normal' $true '2026-09-03 23:30' $cfg $mBroken 'un prompt' | Out-Null

$mNoDate = New-MarkerPath 'sans-date'
Set-Content -Path $mNoDate -Encoding ascii -Value '{ "armedAt": "2026-09-03T22:50:00", "until": "banane" }'
Test-Warning 'marqueur sans date lisible -> couvre-feu normal' $true '2026-09-03 23:30' $cfg $mNoDate 'un prompt' | Out-Null

# Sans couvre-feu configure, il n y a rien a desarmer.
$mNoCfg = New-MarkerPath 'sans-config'
Test-Warning 'config absente + #jedors -> aucun message' $false '2026-09-03 23:30' $missing $mNoCfg '#jedors' | Out-Null
Write-Result (-not (Test-Path $mNoCfg)) `
    'config absente : aucun marqueur ecrit' "marqueur=$mNoCfg"

$mDisabled = New-MarkerPath 'desactive'
Test-Warning 'enabled=false + #jedors -> aucun message' $false '2026-09-03 23:30' $disabled $mDisabled '#jedors' | Out-Null

Write-Host ''
Write-Host 'Cablage settings.json : UserPromptSubmit uniquement' -ForegroundColor Cyan

$settingsFiles = @(
    @{ Label = 'repo';      Path = (Join-Path $RepoRoot 'claude-code\settings.json') },
    @{ Label = '~/.claude'; Path = (Join-Path $env:USERPROFILE '.claude\settings.json') }
)

foreach ($sf in $settingsFiles) {
    if (-not (Test-Path $sf.Path)) {
        Write-Result $false ("settings.json ({0}) introuvable" -f $sf.Label) $sf.Path
        continue
    }
    $settings = Get-Content $sf.Path -Raw | ConvertFrom-Json
    $hooks = $settings.hooks

    $allCommands = @()
    foreach ($evt in $hooks.PSObject.Properties) {
        foreach ($matcher in @($evt.Value)) {
            foreach ($h in @($matcher.hooks)) {
                $allCommands += [pscustomobject]@{ Event = $evt.Name; Command = [string]$h.command }
            }
        }
    }

    $curfewEntries = @($allCommands | Where-Object { $_.Command -match 'curfew' })
    Write-Result ($curfewEntries.Count -eq 1) `
        ("settings.json ({0}) : le hook curfew est enregistre une seule fois" -f $sf.Label) `
        ("trouve : " + (($curfewEntries | ForEach-Object { $_.Event }) -join ', '))

    Write-Result (@($curfewEntries | Where-Object { $_.Event -ne 'UserPromptSubmit' }).Count -eq 0) `
        ("settings.json ({0}) : curfew branche sur UserPromptSubmit uniquement" -f $sf.Label) `
        ("evenements : " + (($curfewEntries | ForEach-Object { $_.Event }) -join ', '))

    foreach ($forbidden in @('PreToolUse', 'PostToolUse', 'Stop', 'SessionStart', 'SubagentStop', 'PreCompact')) {
        $hit = @($allCommands | Where-Object { $_.Event -eq $forbidden -and $_.Command -match 'curfew' })
        Write-Result ($hit.Count -eq 0) `
            ("settings.json ({0}) : aucun curfew sur {1} (ne coupe jamais une tache active)" -f $sf.Label, $forbidden) `
            'un hook curfew sur cet evenement interromprait le travail en cours'
    }

    Write-Result (@($allCommands | Where-Object { $_.Command -match 'error-vault-trigger' }).Count -ge 1) `
        ("settings.json ({0}) : le hook error-vault-trigger est preserve" -f $sf.Label) `
        'le hook existant a disparu de la configuration'
}

Write-Host ''
Write-Host 'Le hook n attend jamais sur stdin (rien ne doit pouvoir le suspendre)' -ForegroundColor Cyan
$hookSource = Get-Content $HookPath -Raw -ErrorAction SilentlyContinue
Write-Result ($hookSource -notmatch 'Read-Host') `
    'curfew.ps1 ne demande jamais de saisie interactive' 'Read-Host suspendrait le hook'
Write-Result ($hookSource -match 'IsInputRedirected') `
    'curfew.ps1 ne lit le payload que si stdin est redirige' `
    'sans ce garde-fou, une entree restee sur la console ferait attendre le hook'
Write-Result ($hookSource -notmatch '(?ms)ReadToEnd.*?IsInputRedirected') `
    'le garde-fou precede la lecture de stdin' 'IsInputRedirected doit etre teste AVANT ReadToEnd'
Write-Result ($hookSource -notmatch '(?m)^\s*exit\s+2\b') `
    'curfew.ps1 ne peut plus emettre le code de blocage (exit 2)' 'le hook doit avertir, jamais bloquer'

Remove-Item $TmpDir -Recurse -Force -ErrorAction SilentlyContinue

$color = 'Green'
if ($script:Fail -gt 0) { $color = 'Red' }
Write-Host ''
Write-Host ("Resultat : {0} pass / {1} fail" -f $script:Pass, $script:Fail) -ForegroundColor $color
if ($script:Fail -gt 0) { exit 1 }
exit 0
