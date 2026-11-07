$ErrorActionPreference = 'Stop'
New-Item -Type Directory -Force probe-results | Out-Null
$installer = Join-Path $env:RUNNER_TEMP 'EAappInstaller.exe'
Invoke-WebRequest 'https://origin-a.akamaihd.net/EA-Desktop-Client-Download/installer-releases/EAappInstaller.exe' -OutFile $installer
if ((Get-AuthenticodeSignature $installer).Status -ne 'Valid') { throw 'Invalid EA installer signature' }
$setup = Start-Process $installer -ArgumentList '/quiet','/norestart' -PassThru
if (-not $setup.WaitForExit(360000)) { throw 'EA installation timed out' }
"Installer exit: $($setup.ExitCode)" | Out-File probe-results/setup.txt
$exe = Join-Path $env:ProgramFiles 'Electronic Arts/EA Desktop/EA Desktop/EADesktop.exe'
if (-not (Test-Path $exe)) { throw 'EA client was not installed' }
# The installer starts a client automatically; restart that disposable process
# so it receives the interface flags on its first process invocation.
Get-Process EADesktop -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 2
$client = Start-Process $exe -ArgumentList '--webDebugEnabled','--webDebugPort','9394' -PassThru
function WebSocketMessage($Socket, $Value) {
    $bytes = [Text.Encoding]::UTF8.GetBytes(($Value | ConvertTo-Json -Depth 30 -Compress))
    $Socket.SendAsync([ArraySegment[byte]]$bytes,[Net.WebSockets.WebSocketMessageType]::Text,$true,[Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $buffer = New-Object byte[] 1048576
    $stream = [IO.MemoryStream]::new()
    do {
        $timeout = [Threading.CancellationTokenSource]::new(10000)
        $read = $Socket.ReceiveAsync([ArraySegment[byte]]$buffer,$timeout.Token).GetAwaiter().GetResult()
        $stream.Write($buffer,0,$read.Count)
    } while (-not $read.EndOfMessage)
    [Text.Encoding]::UTF8.GetString($stream.ToArray()) | ConvertFrom-Json -AsHashtable
}
$results = @{}
for ($attempt = 0; $attempt -lt 60; $attempt++) {
    Start-Sleep 2
    foreach ($port in 4695,9394) {
        $test = [Net.Sockets.TcpClient]::new()
        try {
            $connected = $test.ConnectAsync('127.0.0.1',$port).Wait(500)
            if ($connected -and $test.Connected) { $results["port$port"] = $true }
        } catch {} finally { $test.Dispose() }
    }
    if ($results.port9394) { break }
}
$owners = @(Get-Process EADesktop,EACefSubProcess -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id)
Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $_.OwningProcess -in $owners } | Select-Object LocalAddress,LocalPort,OwningProcess | ConvertTo-Json | Out-File probe-results/listeners.json
$results | ConvertTo-Json | Out-File probe-results/ports.json
if ($results.port4695) {
    $socket = [Net.WebSockets.ClientWebSocket]::new()
    $socket.ConnectAsync([Uri]'ws://127.0.0.1:4695',[Threading.CancellationToken]::None).GetAwaiter().GetResult()
    $schema = WebSocketMessage $socket @{type=3;id=1}
    $objects = @{}
    foreach ($object in $schema.data.Keys) {
        $meta = $schema.data[$object]
        $objects[$object] = @{ methods=$meta.methods; signals=$meta.signals; enums=$meta.enums }
    }
    $objects | ConvertTo-Json -Depth 30 | Out-File probe-results/qt-interface.json
    $socket.Dispose()
}
if ($results.port9394) {
    $pages = Invoke-RestMethod 'http://127.0.0.1:9394/json/list'
    $index = 0
    foreach ($page in $pages) {
        if ($page.type -ne 'page') { continue }
        $socket = [Net.WebSockets.ClientWebSocket]::new()
        $socket.ConnectAsync([Uri]$page.webSocketDebuggerUrl,[Threading.CancellationToken]::None).GetAwaiter().GetResult()
        $expression = 'JSON.stringify({url:location.origin+location.pathname,hasChannel:!!window.webchannel,objects:window.webchannel?Object.fromEntries(Object.entries(window.webchannel.objects).map(([k,v])=>[k,{methods:Object.keys(v).filter(p=>typeof v[p]==="function"),enums:Object.fromEntries(Object.entries(v).filter(([p,x])=>x&&typeof x==="object"&&Object.values(x).every(y=>typeof y==="number")))}])):{}})'
        $response = WebSocketMessage $socket @{id=1;method='Runtime.evaluate';params=@{expression=$expression;returnByValue=$true;awaitPromise=$true}}
        $response | ConvertTo-Json -Depth 30 | Out-File "probe-results/page-$index.json"
        $socket.Dispose()
        $index++
    }
}
if (-not $results.port4695 -and -not $results.port9394) {
    $logs = Join-Path $env:ProgramData 'EA Desktop/Logs'
    if (Test-Path $logs) {
        Get-ChildItem $logs -Filter EADesktop.log | ForEach-Object {
            Get-Content $_.FullName | Select-String 'FATAL|Command line|Version:|startup.*error|[Ee][Rr][Rr][Oo][Rr]|webDebug|[Dd]ebug.*port|[Uu]nrecognized.*option' | ForEach-Object { $_.Line } | Out-File probe-results/startup.txt
        }
    }
    throw 'EA interface did not become available'
}
