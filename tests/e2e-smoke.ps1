$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $PSScriptRoot
$Port = if ($env:TAURI_WD_TEST_PORT) { $env:TAURI_WD_TEST_PORT } else { "4464" }
$StartupTimeout = if ($env:TAURI_WD_TEST_STARTUP_TIMEOUT) {
    $env:TAURI_WD_TEST_STARTUP_TIMEOUT
} else {
    "90"
}
$TargetDir = Join-Path $Root "target"
$Driver = Join-Path $TargetDir "debug/tauri-wd.exe"
$App = Join-Path $TargetDir "debug/webdriver-fixture.exe"
$BaseUrl = "http://127.0.0.1:$Port"
$SessionId = $null
$DriverProcess = $null
$UploadFile = Join-Path ([System.IO.Path]::GetTempPath()) "tauri-wd-upload-$PID.txt"
$ScreenshotFile = Join-Path ([System.IO.Path]::GetTempPath()) "tauri-wd-shot-$PID.png"
[System.IO.File]::WriteAllText($UploadFile, "webdriver-file-upload")

function Invoke-ErrorBody {
    param([string]$Method, [string]$Uri, [string]$Body)
    $Response = Invoke-WebRequest -Method $Method -Uri $Uri `
        -ContentType "application/json" -Body $Body -SkipHttpErrorCheck
    return $Response.Content
}

function Find-Css {
    param([string]$Selector)
    $Request = @{ using = "css selector"; value = $Selector } | ConvertTo-Json
    $Response = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element" `
        -ContentType "application/json" -Body $Request
    return $Response.value."element-6066-11e4-a52e-4f735466cecf"
}

function Execute-Script {
    param([string]$Script)
    $Request = @{ script = $Script; args = @() } | ConvertTo-Json
    return Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/sync" `
        -ContentType "application/json" -Body $Request
}

try {
    cargo build --locked --manifest-path (Join-Path $Root "Cargo.toml") `
        --package tauri-cross-platform-webdriver
    if ($LASTEXITCODE -ne 0) { throw "Driver build failed" }

    $env:CARGO_TARGET_DIR = $TargetDir
    cargo build --locked `
        --manifest-path (Join-Path $Root "tests/fixture/src-tauri/Cargo.toml")
    if ($LASTEXITCODE -ne 0) { throw "Fixture build failed" }

    $DriverProcess = Start-Process -FilePath $Driver `
        -ArgumentList "--port", $Port, "--startup-timeout", $StartupTimeout, "--log", "info" `
        -PassThru -NoNewWindow

    $Ready = $false
    for ($Attempt = 0; $Attempt -lt 100; $Attempt++) {
        try {
            Invoke-RestMethod -Uri "$BaseUrl/status" | Out-Null
            $Ready = $true
            break
        } catch {
            Start-Sleep -Milliseconds 50
        }
    }
    if (-not $Ready) { throw "tauri-wd did not start" }

    $Capabilities = @{
        capabilities = @{
            alwaysMatch = @{
                "tauri:options" = @{ application = $App }
            }
        }
    } | ConvertTo-Json -Depth 6
    $Session = Invoke-RestMethod -Method Post -Uri "$BaseUrl/session" `
        -ContentType "application/json" -Body $Capabilities
    $SessionId = $Session.value.sessionId
    if (-not $SessionId) { throw "Session was not created" }

    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/timeouts" `
        -ContentType "application/json" `
        -Body '{"implicit":null,"pageLoad":null,"script":null}' | Out-Null
    $NullTimeouts = Invoke-RestMethod -Uri "$BaseUrl/session/$SessionId/timeouts"
    if ($null -ne $NullTimeouts.value.implicit -or
        $null -ne $NullTimeouts.value.pageLoad -or
        $null -ne $NullTimeouts.value.script) {
        throw "Null timeouts were not preserved"
    }
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/timeouts" `
        -ContentType "application/json" `
        -Body '{"implicit":1000.0,"pageLoad":300000.0,"script":2.0}' | Out-Null
    $FloatTimeouts = Invoke-RestMethod -Uri "$BaseUrl/session/$SessionId/timeouts"
    if ($FloatTimeouts.value.implicit -ne 1000 -or
        $FloatTimeouts.value.pageLoad -ne 300000 -or
        $FloatTimeouts.value.script -ne 2) {
        throw "Integral floating-point timeouts were not accepted"
    }
    $InvalidTimeouts = Invoke-ErrorBody -Method Post `
        -Uri "$BaseUrl/session/$SessionId/timeouts" `
        -Body '{"implicit":2000,"pageLoad":-1,"script":40}'
    if ($InvalidTimeouts -notlike '*"error":"invalid argument"*') {
        throw "Invalid timeout was accepted"
    }
    $AtomicTimeouts = Invoke-RestMethod -Uri "$BaseUrl/session/$SessionId/timeouts"
    if ($AtomicTimeouts.value.implicit -ne 1000 -or
        $AtomicTimeouts.value.pageLoad -ne 300000 -or
        $AtomicTimeouts.value.script -ne 2) {
        throw "Invalid timeout request partially changed timeout state"
    }

    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/timeouts" `
        -ContentType "application/json" -Body '{"script":20}' | Out-Null
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/timeouts" `
        -ContentType "application/json" -Body '{"script":null,"implicit":1000}' | Out-Null
    $Timeouts = Invoke-RestMethod -Uri "$BaseUrl/session/$SessionId/timeouts"
    if ($null -ne $Timeouts.value.script) { throw "script:null was not preserved" }
    $AsyncAfterNull = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/async" `
        -ContentType "application/json" `
        -Body '{"script":"var done=arguments[arguments.length-1];setTimeout(function(){done(\"after-null\");},75);","args":[]}'
    if ($AsyncAfterNull.value -ne "after-null") { throw "script:null still imposed a deadline" }
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/timeouts" `
        -ContentType "application/json" -Body '{"script":20}' | Out-Null
    $AsyncTimeout = Invoke-ErrorBody -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/async" `
        -Body '{"script":"var done=arguments[arguments.length-1];setTimeout(function(){done(\"late\");},75);","args":[]}'
    if ($AsyncTimeout -notlike '*"error":"script timeout"*') {
        throw "Finite script timeout was not restored"
    }
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/timeouts" `
        -ContentType "application/json" -Body '{"script":null}' | Out-Null

    Execute-Script 'setTimeout(function(){var e=document.createElement("span");e.id="implicit-single";document.body.appendChild(e);},200);return null;' | Out-Null
    if (-not (Find-Css "#implicit-single")) { throw "Implicit single-element wait failed" }

    Execute-Script 'setTimeout(function(){for(var i=0;i<2;i++){var e=document.createElement("span");e.className="implicit-many";document.body.appendChild(e);}},200);return null;' | Out-Null
    $ManyRequest = @{ using = "css selector"; value = ".implicit-many" } | ConvertTo-Json
    $Many = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/elements" `
        -ContentType "application/json" -Body $ManyRequest
    if ($Many.value.Count -ne 2) { throw "Implicit multi-element wait failed" }

    $ParentId = Find-Css "#late-parent"
    Execute-Script 'document.querySelector("#late-parent").innerHTML="";setTimeout(function(){document.querySelector("#late-parent").innerHTML="<i class=from-parent-single></i>";},200);return null;' | Out-Null
    $ParentRequest = @{ using = "css selector"; value = ".from-parent-single" } | ConvertTo-Json
    $ParentOne = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$ParentId/element" `
        -ContentType "application/json" -Body $ParentRequest
    if (-not $ParentOne.value."element-6066-11e4-a52e-4f735466cecf") {
        throw "Implicit child-element wait failed"
    }
    Execute-Script 'document.querySelector("#late-parent").innerHTML="";setTimeout(function(){document.querySelector("#late-parent").innerHTML="<i class=from-parent-many></i><i class=from-parent-many></i>";},200);return null;' | Out-Null
    $ParentRequest = @{ using = "css selector"; value = ".from-parent-many" } | ConvertTo-Json
    $ParentMany = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$ParentId/elements" `
        -ContentType "application/json" -Body $ParentRequest
    if ($ParentMany.value.Count -ne 2) { throw "Child element collection failed" }

    $ShadowHostId = Find-Css "#shadow"
    $Shadow = Invoke-RestMethod `
        -Uri "$BaseUrl/session/$SessionId/element/$ShadowHostId/shadow"
    $ShadowId = $Shadow.value."shadow-6066-11e4-a52e-4f735466cecf"
    Execute-Script 'var root=document.querySelector("#shadow").shadowRoot;setTimeout(function(){root.innerHTML="<b class=from-shadow-single></b>";},200);return null;' | Out-Null
    $ShadowRequest = @{ using = "css selector"; value = ".from-shadow-single" } | ConvertTo-Json
    $ShadowOne = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/shadow/$ShadowId/element" `
        -ContentType "application/json" -Body $ShadowRequest
    if (-not $ShadowOne.value."element-6066-11e4-a52e-4f735466cecf") {
        throw "Implicit shadow-element wait failed"
    }
    Execute-Script 'var root=document.querySelector("#shadow").shadowRoot;root.innerHTML="";setTimeout(function(){root.innerHTML="<b class=from-shadow-many></b><b class=from-shadow-many></b>";},200);return null;' | Out-Null
    $ShadowRequest = @{ using = "css selector"; value = ".from-shadow-many" } | ConvertTo-Json
    $ShadowMany = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/shadow/$ShadowId/elements" `
        -ContentType "application/json" -Body $ShadowRequest
    if ($ShadowMany.value.Count -ne 2) { throw "Shadow element collection failed" }

    $ElementRequest = @{
        using = "css selector"
        value = "#increment"
    } | ConvertTo-Json
    $Element = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element" `
        -ContentType "application/json" -Body $ElementRequest
    $ElementId = $Element.value."element-6066-11e4-a52e-4f735466cecf"
    if (-not $ElementId) { throw "Element was not found" }

    for ($Attempt = 0; $Attempt -lt 40; $Attempt++) {
        $ExecuteRequest = @{
            script = "return document.contains(arguments[0].nested[0]);"
            args = @(@{
                nested = @(@{
                    "element-6066-11e4-a52e-4f735466cecf" = $ElementId
                })
            })
        } | ConvertTo-Json -Depth 8
        $Result = Invoke-RestMethod -Method Post `
            -Uri "$BaseUrl/session/$SessionId/execute/sync" `
            -ContentType "application/json" -Body $ExecuteRequest
        if ($Result.value -ne $true) { throw "Element argument was not resolved" }
    }

    $ReturnedElement = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/sync" `
        -ContentType "application/json" `
        -Body '{"script":"return {nested:[document.querySelector(\"#increment\")]};","args":[]}'
    $ReturnedElementId = $ReturnedElement.value.nested[0]."element-6066-11e4-a52e-4f735466cecf"
    if (-not $ReturnedElementId) { throw "Returned DOM element was not serialized" }

    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$ReturnedElementId/click" `
        -ContentType "application/json" -Body "{}" | Out-Null
    $Count = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/sync" `
        -ContentType "application/json" `
        -Body '{"script":"return document.querySelector(\"#count\").textContent;","args":[]}'
    if ($Count.value -ne "1") { throw "Returned DOM element could not be reused" }

    $ClickId = Find-Css "#click-sequence"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$ClickId/click" `
        -ContentType "application/json" -Body "{}" | Out-Null
    $ClickEvents = Execute-Script 'return document.querySelector("#click-result").textContent;'
    if ($ClickEvents.value -ne "pointerdown,mousedown,focus,pointerup,mouseup,click") {
        throw "Element click event sequence was incorrect"
    }

    $CaptureClickId = Find-Css "#capture-click"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$CaptureClickId/click" `
        -ContentType "application/json" -Body "{}" | Out-Null
    $CaptureClicks = Execute-Script "return window.captureClicks;"
    if ($CaptureClicks.value -ne 1) { throw "Pointer capture click failed" }

    foreach ($Case in @(
        @{ Selector = "#obscured"; Error = "element click intercepted"; Message = "center point is obscured" },
        @{ Selector = "#hidden-button"; Error = "element not interactable"; Message = "element has no in-view center point" },
        @{ Selector = "#no-pointer-button"; Error = "element not interactable"; Message = "element does not receive pointer events" },
        @{ Selector = "#invisible-button"; Error = "element not interactable"; Message = "center point is outside the pointer-interactable paint tree" }
    )) {
        $ErrorElementId = Find-Css $Case.Selector
        $ErrorBody = Invoke-ErrorBody -Method Post `
            -Uri "$BaseUrl/session/$SessionId/element/$ErrorElementId/click" -Body "{}"
        if (
            $ErrorBody -notlike "*`"error`":`"$($Case.Error)`"*" -or
            $ErrorBody -notlike "*$($Case.Message)*"
        ) {
            throw "$($Case.Selector) returned the wrong WebDriver error: $ErrorBody"
        }
    }

    $CheckboxId = Find-Css "#checkbox"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$CheckboxId/click" `
        -ContentType "application/json" -Body "{}" | Out-Null
    $CheckboxState = Execute-Script 'return document.querySelector("#checkbox").checked;'
    if ($CheckboxState.value -ne $true) { throw "Checkbox default activation failed" }

    $DisabledId = Find-Css "#disabled-button"
    Execute-Script 'window.disabledClicks=0;document.querySelector("#disabled-button").addEventListener("click",function(){window.disabledClicks++;});return null;' | Out-Null
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$DisabledId/click" `
        -ContentType "application/json" -Body "{}" | Out-Null
    $DisabledClicks = Execute-Script "return window.disabledClicks;"
    if ($DisabledClicks.value -ne 0) { throw "Disabled element was activated" }

    $KeysId = Find-Css "#keys"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$KeysId/value" `
        -ContentType "application/json" -Body '{"text":"ab\uE012\uE003Z"}' | Out-Null
    $KeyValue = Execute-Script 'return document.querySelector("#keys").value;'
    if ($KeyValue.value -ne "Zb") { throw "Special-key editing was incorrect" }
    $KeyEvents = Execute-Script 'return document.querySelector("#key-result").textContent;'
    if ($KeyEvents.value -notlike "*keydown:ArrowLeft*" -or
        $KeyEvents.value -notlike "*keydown:Backspace*") {
        throw "Special-key events were not dispatched"
    }

    Execute-Script 'var el=document.querySelector("#keys");el.value="abcd";el.focus();el.setSelectionRange(4,4);window.keyDetails=[];return null;' | Out-Null
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$KeysId/value" `
        -ContentType "application/json" -Body '{"text":"\uE008\uE012\uE000X"}' | Out-Null
    $ShiftValue = Execute-Script 'return document.querySelector("#keys").value;'
    if ($ShiftValue.value -ne "abcX") { throw "Shift selection anchor was incorrect" }

    Execute-Script 'var el=document.querySelector("#keys");el.value="";el.focus();window.keyDetails=[];return null;' | Out-Null
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$KeysId/value" `
        -ContentType "application/json" `
        -Body '{"text":"\uE050a\uE000\uE007\uE054"}' | Out-Null
    $KeyDetails = Execute-Script "return window.keyDetails;"
    $KeyDetailText = $KeyDetails.value -join ","
    if ($KeyDetailText -notlike "*keydown:Shift:ShiftRight:2*" -or
        $KeyDetailText -notlike "*keydown:Enter:NumpadEnter:1*" -or
        $KeyDetailText -notlike "*keydown:PageUp:Numpad9:3*") {
        throw "Extended WebDriver key metadata was incorrect"
    }

    Execute-Script 'document.querySelector("#keys").value="";document.querySelector("#keys-next").value="";return null;' | Out-Null
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$KeysId/value" `
        -ContentType "application/json" -Body '{"text":"\uE004x"}' | Out-Null
    $TabValue = Execute-Script 'return [document.activeElement.id,document.querySelector("#keys-next").value];'
    if (($TabValue.value -join ",") -ne "keys-next,x") {
        throw "Tab did not retarget subsequent keys"
    }

    $DelayedKeysId = Find-Css "#delayed-keys"
    Execute-Script 'setTimeout(function(){document.querySelector("#delayed-keys").style.display="inline";},200);return null;' | Out-Null
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$DelayedKeysId/value" `
        -ContentType "application/json" -Body '{"text":"ready"}' | Out-Null
    $DelayedValue = Execute-Script 'return document.querySelector("#delayed-keys").value;'
    if ($DelayedValue.value -ne "ready") { throw "Send Keys did not wait for interactability" }

    $ReadonlyId = Find-Css "#readonly"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$ReadonlyId/value" `
        -ContentType "application/json" -Body '{"text":"x"}' | Out-Null
    $ReadonlyValue = Execute-Script 'return document.querySelector("#readonly").value;'
    if ($ReadonlyValue.value -ne "fixed") {
        throw "Read-only input value changed"
    }

    $BodyId = Find-Css "body"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$BodyId/value" `
        -ContentType "application/json" -Body '{"text":"q"}' | Out-Null
    $BodyActive = Execute-Script "return document.activeElement===document.body;"
    if ($BodyActive.value -ne $true) { throw "Body did not remain the active element" }

    $HtmlId = Find-Css "html"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$HtmlId/value" `
        -ContentType "application/json" -Body '{"text":"q"}' | Out-Null
    $HtmlActive = Execute-Script "return document.activeElement===document.documentElement;"
    if ($HtmlActive.value -ne $true) { throw "Root element did not become active" }

    $OpacityKeysId = Find-Css "#opacity-keys"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$OpacityKeysId/value" `
        -ContentType "application/json" -Body '{"text":"q"}' | Out-Null
    $OpacityValue = Execute-Script 'return document.querySelector("#opacity-keys").value;'
    if ($OpacityValue.value -ne "q") { throw "Transparent input did not receive keys" }

    $ObscuredId = Find-Css "#obscured"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$ObscuredId/value" `
        -ContentType "application/json" -Body '{"text":"q"}' | Out-Null

    $DateId = Find-Css "#date"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$DateId/value" `
        -ContentType "application/json" -Body '{"text":"01/02/2020"}' | Out-Null
    $DateValue = Execute-Script 'return document.querySelector("#date").value;'
    if ($DateValue.value -ne "2020-01-02") { throw "Date input typing failed" }

    $FrameId = Find-Css "#frame"
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$FrameId/value" `
        -ContentType "application/json" -Body '{"text":"frame"}' | Out-Null
    $FrameInputValue = Execute-Script 'return document.querySelector("#frame").contentDocument.querySelector("#frame-input").value;'
    if ($FrameInputValue.value -ne "frame") { throw "Iframe Send Keys failed" }

    Execute-Script 'window.actionEvents=[];document.querySelector("#keys").focus();return null;' | Out-Null
    $TickActions = @{
        actions = @(
            @{
                type = "key"
                id = "a"
                actions = @(
                    @{ type = "keyDown"; value = "a" },
                    @{ type = "keyUp"; value = "a" }
                )
            },
            @{
                type = "key"
                id = "b"
                actions = @(
                    @{ type = "keyDown"; value = "b" },
                    @{ type = "keyUp"; value = "b" }
                )
            }
        )
    } | ConvertTo-Json -Depth 8
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/actions" `
        -ContentType "application/json" -Body $TickActions | Out-Null
    $ActionEvents = Execute-Script "return window.actionEvents;"
    if (($ActionEvents.value -join ",") -ne "keydown:a,keydown:b,keyup:a,keyup:b") {
        throw "Action sources were not dispatched tick-by-tick"
    }

    Execute-Script "window.keyDetails=[];return null;" | Out-Null
    $SpecialActions = '{"actions":[{"type":"key","id":"special","actions":[{"type":"keyDown","value":"\uE050"},{"type":"keyDown","value":"\uE054"},{"type":"keyUp","value":"\uE054"},{"type":"keyUp","value":"\uE050"},{"type":"keyDown","value":"\uE006"},{"type":"keyUp","value":"\uE006"},{"type":"keyDown","value":"\uE007"},{"type":"keyUp","value":"\uE007"}]}]}'
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/actions" `
        -ContentType "application/json" -Body $SpecialActions | Out-Null
    $ActionKeyDetails = Execute-Script "return window.keyDetails;"
    $ActionKeyDetailsText = $ActionKeyDetails.value -join ","
    foreach ($Expected in @(
        "keydown:Shift:ShiftRight:2",
        "keydown:PageUp:Numpad9:3",
        "keydown:Enter:Enter:0",
        "keydown:Enter:NumpadEnter:1"
    )) {
        if ($ActionKeyDetailsText -notlike "*$Expected*") {
            throw "Action special-key mapping failed for $Expected"
        }
    }

    Execute-Script "window.actionEvents=[];return null;" | Out-Null
    $KeyUpOnlyActions = '{"actions":[{"type":"key","id":"keyup-only","actions":[{"type":"keyUp","value":"z"}]}]}'
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/actions" `
        -ContentType "application/json" -Body $KeyUpOnlyActions | Out-Null
    $KeyUpOnlyEvents = Execute-Script "return window.actionEvents;"
    if ($KeyUpOnlyEvents.value.Count -ne 0) {
        throw "Unpressed keyUp dispatched an event"
    }

    $ScreenshotId = Find-Css "#screenshot-target"
    $Screenshot = Invoke-RestMethod `
        -Uri "$BaseUrl/session/$SessionId/element/$ScreenshotId/screenshot"
    [System.IO.File]::WriteAllBytes(
        $ScreenshotFile,
        [Convert]::FromBase64String($Screenshot.value)
    )
    $Png = [System.IO.File]::ReadAllBytes($ScreenshotFile)
    $ScreenshotWidth = [System.Net.IPAddress]::NetworkToHostOrder(
        [BitConverter]::ToInt32($Png, 16)
    )
    $ScreenshotHeight = [System.Net.IPAddress]::NetworkToHostOrder(
        [BitConverter]::ToInt32($Png, 20)
    )
    if ($ScreenshotWidth -lt 32 -or $ScreenshotWidth -gt 256 -or
        $ScreenshotHeight -lt 16 -or $ScreenshotHeight -gt 128 -or
        $ScreenshotWidth -le $ScreenshotHeight) {
        throw "Element screenshot was not cropped"
    }

    $FrameRequest = @{
        id = @{ "element-6066-11e4-a52e-4f735466cecf" = $FrameId }
    } | ConvertTo-Json -Depth 4
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/frame" `
        -ContentType "application/json" -Body $FrameRequest | Out-Null
    $FrameShotId = Find-Css "#frame-shot"
    $FrameScreenshot = Invoke-RestMethod `
        -Uri "$BaseUrl/session/$SessionId/element/$FrameShotId/screenshot"
    [System.IO.File]::WriteAllBytes(
        $ScreenshotFile,
        [Convert]::FromBase64String($FrameScreenshot.value)
    )
    $FramePng = [System.IO.File]::ReadAllBytes($ScreenshotFile)
    $FrameShotWidth = [System.Net.IPAddress]::NetworkToHostOrder(
        [BitConverter]::ToInt32($FramePng, 16)
    )
    $FrameShotHeight = [System.Net.IPAddress]::NetworkToHostOrder(
        [BitConverter]::ToInt32($FramePng, 20)
    )
    if ($FrameShotWidth -lt 20 -or $FrameShotWidth -gt 160 -or
        $FrameShotHeight -lt 10 -or $FrameShotHeight -gt 80) {
        throw "Iframe element screenshot was not cropped"
    }
    $FrameColorRequest = @{
        script = "var done=arguments[arguments.length-1];var image=new Image();image.onload=function(){var canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;var context=canvas.getContext('2d');context.drawImage(image,0,0);done(Array.from(context.getImageData(Math.floor(image.width/2),Math.floor(image.height/2),1,1).data));};image.onerror=function(){done('error');};image.src='data:image/png;base64,'+arguments[0];"
        args = @($FrameScreenshot.value)
    } | ConvertTo-Json -Depth 4
    $FrameShotColor = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/async" `
        -ContentType "application/json" -Body $FrameColorRequest
    if (($FrameShotColor.value -join ",") -ne "204,0,0,255") {
        throw "Iframe element screenshot used the wrong top-level crop"
    }
    Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/frame/parent" `
        -ContentType "application/json" -Body "{}" | Out-Null

    $UploadRequest = @{
        using = "css selector"
        value = "#upload"
    } | ConvertTo-Json
    $UploadElement = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element" `
        -ContentType "application/json" -Body $UploadRequest
    $UploadElementId = $UploadElement.value."element-6066-11e4-a52e-4f735466cecf"
    if (-not $UploadElementId) { throw "File input was not found" }
    $UploadClickError = Invoke-ErrorBody -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$UploadElementId/click" -Body "{}"
    if ($UploadClickError -notlike '*"error":"invalid argument"*') {
        throw "File input click did not return invalid argument"
    }
    $UploadKeys = @{
        text = $UploadFile
        value = @()
    } | ConvertTo-Json
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element/$UploadElementId/value" `
        -ContentType "application/json" -Body $UploadKeys | Out-Null
    $UploadResult = $null
    for ($Attempt = 0; $Attempt -lt 40; $Attempt++) {
        $UploadResult = Invoke-RestMethod -Method Post `
            -Uri "$BaseUrl/session/$SessionId/execute/sync" `
            -ContentType "application/json" `
            -Body '{"script":"return document.querySelector(\"#upload-result\").textContent;","args":[]}'
        if ($UploadResult.value -like "*webdriver-file-upload*") { break }
        Start-Sleep -Milliseconds 50
    }
    if ($UploadResult.value -notlike "*webdriver-file-upload*") {
        throw "File input did not receive the selected file"
    }

    $PointerRequest = @{
        using = "css selector"
        value = "#pointer-pad"
    } | ConvertTo-Json
    $PointerElement = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/element" `
        -ContentType "application/json" -Body $PointerRequest
    $PointerElementId = $PointerElement.value."element-6066-11e4-a52e-4f735466cecf"
    if (-not $PointerElementId) { throw "Pointer pad was not found" }
    $PointerActions = @{
        actions = @(@{
            type = "pointer"
            id = "mouse"
            actions = @(
                @{
                    type = "pointerMove"
                    x = 0
                    y = 0
                    origin = @{
                        "element-6066-11e4-a52e-4f735466cecf" = $PointerElementId
                    }
                },
                @{ type = "pointerDown"; button = 0 },
                @{ type = "pause"; duration = 50 },
                @{ type = "pointerMove"; x = 25; y = 10; origin = "pointer" },
                @{ type = "pointerUp"; button = 0 }
            )
        })
    } | ConvertTo-Json -Depth 8
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/actions" `
        -ContentType "application/json" -Body $PointerActions | Out-Null
    $PointerResult = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/sync" `
        -ContentType "application/json" `
        -Body '{"script":"return document.querySelector(\"#pointer-result\").textContent;","args":[]}'
    if ($PointerResult.value -ne "move:25,10:1:up:0") {
        throw "Pointer actions did not emit pointer drag events"
    }

    Execute-Script "window.pointerMoveCount=0;return null;" | Out-Null
    $InterpolatedPointerActions = @{
        actions = @(@{
            type = "pointer"
            id = "interpolated-mouse"
            actions = @(
                @{
                    type = "pointerMove"
                    x = 0
                    y = 0
                    origin = @{
                        "element-6066-11e4-a52e-4f735466cecf" = $PointerElementId
                    }
                },
                @{
                    type = "pointerMove"
                    x = 40
                    y = 0
                    duration = 80
                    origin = "pointer"
                }
            )
        })
    } | ConvertTo-Json -Depth 8
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/actions" `
        -ContentType "application/json" -Body $InterpolatedPointerActions | Out-Null
    $PointerMoveCount = Execute-Script "return window.pointerMoveCount;"
    if ($PointerMoveCount.value -le 2) {
        throw "Timed pointer move was not interpolated"
    }

    Execute-Script "window.pointerClickCount=0;return null;" | Out-Null
    $PersistentPointerDown = @{
        actions = @(@{
            type = "pointer"
            id = "persistent-mouse"
            actions = @(
                @{
                    type = "pointerMove"
                    x = 0
                    y = 0
                    origin = @{
                        "element-6066-11e4-a52e-4f735466cecf" = $PointerElementId
                    }
                },
                @{ type = "pointerDown"; button = 0 }
            )
        })
    } | ConvertTo-Json -Depth 8
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/actions" `
        -ContentType "application/json" -Body $PersistentPointerDown | Out-Null
    $PersistentPointerUp = @{
        actions = @(@{
            type = "pointer"
            id = "persistent-mouse"
            actions = @(@{ type = "pointerUp"; button = 0 })
        })
    } | ConvertTo-Json -Depth 8
    Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/actions" `
        -ContentType "application/json" -Body $PersistentPointerUp | Out-Null
    $PointerClickCount = Execute-Script "return window.pointerClickCount;"
    if ($PointerClickCount.value -ne 1) {
        throw "Pointer source state did not persist across action commands"
    }

    $AsyncElement = Invoke-RestMethod -Method Post `
        -Uri "$BaseUrl/session/$SessionId/execute/async" `
        -ContentType "application/json" `
        -Body '{"script":"arguments[arguments.length - 1](document.querySelector(\"#name\"));","args":[]}'
    if (-not $AsyncElement.value."element-6066-11e4-a52e-4f735466cecf") {
        throw "Async DOM element was not serialized"
    }

    for ($Attempt = 0; $Attempt -lt 5; $Attempt++) {
        Invoke-RestMethod -Method Post -Uri "$BaseUrl/session/$SessionId/refresh" `
            -ContentType "application/json" -Body "{}" | Out-Null
        $Title = Invoke-RestMethod -Uri "$BaseUrl/session/$SessionId/title"
        if ($Title.value -ne "WebDriver Fixture") { throw "Refresh failed" }
    }

    Invoke-RestMethod -Method Delete -Uri "$BaseUrl/session/$SessionId" | Out-Null
    $SessionId = $null

    for ($Attempt = 0; $Attempt -lt 4; $Attempt++) {
        $Session = Invoke-RestMethod -Method Post -Uri "$BaseUrl/session" `
            -ContentType "application/json" -Body $Capabilities
        $SessionId = $Session.value.sessionId
        if (-not $SessionId) { throw "Sequential session was not created" }
        $Title = Invoke-RestMethod -Uri "$BaseUrl/session/$SessionId/title"
        if ($Title.value -ne "WebDriver Fixture") { throw "Sequential session was unresponsive" }
        Invoke-RestMethod -Method Delete -Uri "$BaseUrl/session/$SessionId" | Out-Null
        $SessionId = $null
    }

    Write-Output "native WebDriver smoke test passed"
} finally {
    if ($SessionId) {
        try {
            Invoke-RestMethod -Method Delete -Uri "$BaseUrl/session/$SessionId" | Out-Null
        } catch {}
    }
    if ($DriverProcess -and -not $DriverProcess.HasExited) {
        Stop-Process -Id $DriverProcess.Id -Force
        $DriverProcess.WaitForExit()
    }
    Remove-Item -Path $UploadFile -Force -ErrorAction SilentlyContinue
    Remove-Item -Path $ScreenshotFile -Force -ErrorAction SilentlyContinue
}
